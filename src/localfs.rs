//! Local filesystem access.
//!
//! This implementation is stateless. So the easiest way to use it
//! is to create a new instance in your handler every time
//! you need one.

use std::any::Any;
use std::collections::VecDeque;
use std::ffi::OsStr;
use std::future::{self, Future};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::mem;
#[cfg(target_vendor = "apple")]
use std::os::darwin::fs::FileTimesExt;
#[cfg(target_os = "freebsd")]
use std::os::unix::fs::FileTimesExt;
#[cfg(unix)]
use std::os::unix::{
    ffi::OsStrExt,
    fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
};
#[cfg(windows)]
use std::os::windows::fs::FileTimesExt;
#[cfg(target_os = "windows")]
use std::os::windows::prelude::*;
use std::path::{Component, Path, PathBuf};
use std::pin::Pin;
use std::pin::pin;
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::{Buf, Bytes, BytesMut};
use futures_util::{FutureExt, Stream, future::BoxFuture};
use tokio::task;

use libc;
use reflink_copy::reflink_or_copy;

use crate::davpath::DavPath;
use crate::fs::*;
use crate::localfs_macos::DUCacheBuilder;

// Run some code via block_in_place() or spawn_blocking().
//
// There's also a method on LocalFs for this, use the freestanding
// function if you do not want the fs_access_guard() closure to be used.
#[cfg(feature = "localfs")]
#[inline]
async fn blocking<F, R>(func: F) -> R
where
    F: FnOnce() -> R,
    F: Send + 'static,
    R: Send + 'static,
{
    match tokio::runtime::Handle::current().runtime_flavor() {
        tokio::runtime::RuntimeFlavor::MultiThread => task::block_in_place(func),
        _ => task::spawn_blocking(func).await.unwrap(),
    }
}

#[derive(Debug, Clone)]
struct LocalFsMetaData {
    meta: std::fs::Metadata,
    #[cfg(feature = "caldav")]
    is_calendar: bool,
    #[cfg(feature = "carddav")]
    is_addressbook: bool,
}

impl LocalFsMetaData {
    #[allow(unused_variables)]
    fn new(path: &Path, meta: std::fs::Metadata) -> Self {
        #[cfg(any(feature = "caldav", feature = "carddav"))]
        let is_dir = meta.is_dir();
        LocalFsMetaData {
            #[cfg(feature = "caldav")]
            is_calendar: is_dir && path.join(".dav-calendar").exists(),
            #[cfg(feature = "carddav")]
            is_addressbook: is_dir && path.join(".dav-addressbook").exists(),
            meta,
        }
    }

    fn from_file_meta(meta: std::fs::Metadata) -> Self {
        LocalFsMetaData {
            meta,
            #[cfg(feature = "caldav")]
            is_calendar: false,
            #[cfg(feature = "carddav")]
            is_addressbook: false,
        }
    }
}

/// Local Filesystem implementation.
#[derive(Clone)]
pub struct LocalFs {
    pub(crate) inner: Arc<LocalFsInner>,
}

// inner struct.
pub(crate) struct LocalFsInner {
    pub basedir: PathBuf,
    pub canonical_basedir: OnceLock<PathBuf>,
    #[allow(dead_code)]
    pub public: bool,
    pub case_insensitive: bool,
    pub macos: bool,
    pub is_file: bool,
    pub fs_access_guard: Option<Box<dyn Fn() -> Box<dyn Any> + Send + Sync + 'static>>,
}

#[derive(Debug)]
struct LocalFsFile {
    file: Option<std::fs::File>,
    buf: BytesMut,
}

struct LocalFsReadDir {
    fs: LocalFs,
    do_meta: ReadDirMeta,
    buffer: VecDeque<io::Result<LocalFsDirEntry>>,
    dir_cache: Option<DUCacheBuilder>,
    iterator: Option<std::fs::ReadDir>,
    fut: Option<BoxFuture<'static, ReadDirBatch>>,
}

// a DirEntry either already has the metadata available, or a handle
// to the filesystem so it can call fs.blocking()
enum Meta {
    Data(io::Result<std::fs::Metadata>),
    Fs(LocalFs),
}

/// Open a file or directory with enough access to change timestamps.
fn open_for_times(path: &Path) -> io::Result<std::fs::File> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x02000000;
        std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)
            .or_else(|_| {
                std::fs::OpenOptions::new()
                    .read(true)
                    .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
                    .open(path)
            })
    }
    #[cfg(unix)]
    {
        std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
            .or_else(|_| {
                std::fs::OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NOFOLLOW)
                    .open(path)
            })
    }
    #[cfg(not(any(windows, unix)))]
    {
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .or_else(|_| std::fs::File::open(path))
    }
}

fn init_canonical_basedir(basedir: &Path) -> OnceLock<PathBuf> {
    let lock = OnceLock::new();
    if let Ok(canonical) = std::fs::canonicalize(basedir) {
        let _ = lock.set(canonical);
    }
    lock
}

fn path_is_within(base: &Path, path: &Path) -> bool {
    path.strip_prefix(base).is_ok()
}

// Items from the readdir stream.
struct LocalFsDirEntry {
    meta: Meta,
    entry: std::fs::DirEntry,
}

/// Append a single Normal path component.
///
/// `PathBuf::push` re-parses its argument, so a name like `C:` or `\foo` would
/// replace `basedir` on Windows. Only a standalone `Component::Normal` is safe.
pub(crate) fn push_normal_component(path: &mut PathBuf, name: &OsStr) -> FsResult<()> {
    let mut comps = Path::new(name).components();
    match (comps.next(), comps.next()) {
        (Some(Component::Normal(n)), None) => {
            path.push(n);
            Ok(())
        }
        _ => Err(FsError::Forbidden),
    }
}

/// Join `rel` onto `basedir` component-by-component.
///
/// Prefix, root, `.`, and `..` are forbidden so a DavPath cannot escape the share.
pub(crate) fn join_rel_path(basedir: &Path, rel: &Path) -> FsResult<PathBuf> {
    let mut out = basedir.to_path_buf();
    for component in rel.components() {
        match component {
            Component::Normal(seg) => push_normal_component(&mut out, seg)?,
            _ => return Err(FsError::Forbidden),
        }
    }
    Ok(out)
}

pub(crate) fn join_confined(basedir: &Path, path: &DavPath) -> FsResult<PathBuf> {
    join_rel_path(basedir, path.as_rel_ospath())
}

impl LocalFs {
    /// Create a new LocalFs DavFileSystem, serving "base".
    ///
    /// If "public" is set to true, files and directories are created with
    /// mode 666/777 so the process umask applies (typically 644/755, or
    /// 664/775 with umask 002). Otherwise they are private (mode 600/700).
    ///
    /// If "case_insensitive" is set to true, all filesystem lookups will
    /// be case insensitive. Note that this has a _lot_ of overhead!
    ///
    /// Resolved paths, including those reached via symlinks, must stay under
    /// the canonical basedir. A path that would escape the share is 403.
    pub fn new<P: AsRef<Path>>(
        base: P,
        public: bool,
        case_insensitive: bool,
        macos: bool,
    ) -> Box<LocalFs> {
        let basedir = base.as_ref().to_path_buf();

        let inner = LocalFsInner {
            canonical_basedir: init_canonical_basedir(&basedir),
            basedir,
            public,
            macos,
            case_insensitive,
            is_file: false,
            fs_access_guard: None,
        };
        Box::new({
            LocalFs {
                inner: Arc::new(inner),
            }
        })
    }

    /// Create a new LocalFs DavFileSystem, serving "file".
    ///
    /// This is like `new()`, but it always serves this single file.
    /// The request path is ignored.
    pub fn new_file<P: AsRef<Path>>(file: P, public: bool) -> Box<LocalFs> {
        let basedir = file.as_ref().to_path_buf();
        let inner = LocalFsInner {
            canonical_basedir: init_canonical_basedir(&basedir),
            basedir,
            public,
            macos: false,
            case_insensitive: false,
            is_file: true,
            fs_access_guard: None,
        };
        Box::new({
            LocalFs {
                inner: Arc::new(inner),
            }
        })
    }

    // Like new() but pass in a fs_access_guard hook.
    #[doc(hidden)]
    pub fn new_with_fs_access_guard<P: AsRef<Path>>(
        base: P,
        public: bool,
        case_insensitive: bool,
        macos: bool,
        fs_access_guard: Option<Box<dyn Fn() -> Box<dyn Any> + Send + Sync + 'static>>,
    ) -> Box<LocalFs> {
        let basedir = base.as_ref().to_path_buf();

        let inner = LocalFsInner {
            canonical_basedir: init_canonical_basedir(&basedir),
            basedir,
            public,
            macos,
            case_insensitive,
            is_file: false,
            fs_access_guard,
        };
        Box::new({
            LocalFs {
                inner: Arc::new(inner),
            }
        })
    }

    fn fspath_dbg(&self, path: &DavPath) -> PathBuf {
        let mut pathbuf = self.inner.basedir.clone();
        if !self.inner.is_file {
            pathbuf.push(path.as_rel_ospath());
        }
        pathbuf
    }

    fn fspath(&self, path: &DavPath) -> FsResult<PathBuf> {
        if self.inner.is_file {
            return Ok(self.inner.basedir.clone());
        }
        if self.inner.case_insensitive {
            crate::localfs_windows::resolve(&self.inner.basedir, path)
        } else {
            join_confined(&self.inner.basedir, path)
        }
    }

    fn canonical_basedir(&self) -> &Path {
        self.inner.canonical_basedir.get_or_init(|| {
            std::fs::canonicalize(&self.inner.basedir)
                .unwrap_or_else(|_| self.inner.basedir.clone())
        })
    }

    fn require_within(&self, resolved: &Path) -> FsResult<()> {
        if path_is_within(self.canonical_basedir(), resolved) {
            Ok(())
        } else {
            Err(FsError::Forbidden)
        }
    }

    /// Follow symlinks, then require the resolved path to stay under basedir.
    fn confine_follow(&self, path: &Path) -> FsResult<PathBuf> {
        let resolved = std::fs::canonicalize(path).map_err(FsError::from)?;
        self.require_within(&resolved)?;
        Ok(resolved)
    }

    /// Do not follow the final component. Parent must resolve inside basedir.
    fn confine_leaf(&self, path: &Path) -> FsResult<PathBuf> {
        if self.inner.is_file || path == self.inner.basedir.as_path() {
            let resolved = std::fs::canonicalize(&self.inner.basedir).map_err(FsError::from)?;
            self.require_within(&resolved)?;
            return Ok(resolved);
        }
        let filename = path.file_name().ok_or(FsError::Forbidden)?;
        let parent = path.parent().ok_or(FsError::Forbidden)?;
        let parent_canon = std::fs::canonicalize(parent).map_err(FsError::from)?;
        self.require_within(&parent_canon)?;
        let mut out = parent_canon;
        push_normal_component(&mut out, filename)?;
        Ok(out)
    }

    fn reject_symlink(path: &Path) -> FsResult<()> {
        match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.file_type().is_symlink() => Err(FsError::Forbidden),
            Ok(_) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    // threadpool::blocking() adapter, also runs the before/after hooks.
    #[doc(hidden)]
    pub async fn blocking<F, R>(&self, func: F) -> R
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        let this = self.clone();
        blocking(move || {
            let _guard = this.inner.fs_access_guard.as_ref().map(|f| f());
            func()
        })
        .await
    }

    #[cfg(any(feature = "caldav", feature = "carddav"))]
    fn mark_sidecar<'a>(&'a self, path: &'a DavPath, name: &'a str) -> FsFuture<'a, ()> {
        async move {
            let path = self.fspath(path)?;
            let this = self.clone();
            let name = name.to_string();
            self.blocking(move || {
                let path = this.confine_leaf(&path)?;
                let mut sidecar = path;
                push_normal_component(&mut sidecar, OsStr::new(&name))?;
                std::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .open(&sidecar)
                    .map(|_| ())
                    .map_err(FsError::from)
            })
            .await
        }
        .boxed()
    }
}

// This implementation is basically a bunch of boilerplate to
// wrap the std::fs call in self.blocking() calls.
impl DavFileSystem for LocalFs {
    fn metadata<'a>(&'a self, davpath: &'a DavPath) -> FsFuture<'a, Box<dyn DavMetaData>> {
        async move {
            if let Some(meta) = self.is_virtual(davpath) {
                return Ok(meta);
            }
            let path = self.fspath(davpath)?;
            if self.is_notfound(&path) {
                return Err(FsError::NotFound);
            }
            let this = self.clone();
            self.blocking(move || {
                let path = this.confine_follow(&path)?;
                match std::fs::metadata(&path) {
                    Ok(meta) => {
                        Ok(Box::new(LocalFsMetaData::new(&path, meta)) as Box<dyn DavMetaData>)
                    }
                    Err(e) => Err(e.into()),
                }
            })
            .await
        }
        .boxed()
    }

    fn symlink_metadata<'a>(&'a self, davpath: &'a DavPath) -> FsFuture<'a, Box<dyn DavMetaData>> {
        async move {
            if let Some(meta) = self.is_virtual(davpath) {
                return Ok(meta);
            }
            let path = self.fspath(davpath)?;
            if self.is_notfound(&path) {
                return Err(FsError::NotFound);
            }
            let this = self.clone();
            self.blocking(move || {
                let path = this.confine_leaf(&path)?;
                match std::fs::symlink_metadata(&path) {
                    Ok(meta) => {
                        Ok(Box::new(LocalFsMetaData::new(&path, meta)) as Box<dyn DavMetaData>)
                    }
                    Err(e) => Err(e.into()),
                }
            })
            .await
        }
        .boxed()
    }

    // read_dir is a bit more involved - but not much - than a simple wrapper,
    // because it returns a stream.
    fn read_dir<'a>(
        &'a self,
        davpath: &'a DavPath,
        meta: ReadDirMeta,
    ) -> FsFuture<'a, FsStream<Box<dyn DavDirEntry>>> {
        async move {
            trace!("FS: read_dir {:?}", self.fspath_dbg(davpath));
            let path = self.fspath(davpath)?;
            let this = self.clone();
            let (iter, path2) = self
                .blocking(move || -> FsResult<_> {
                    let path = this.confine_follow(&path)?;
                    let iterator = std::fs::read_dir(&path)?;
                    Ok((iterator, path))
                })
                .await?;
            let strm = LocalFsReadDir {
                fs: self.clone(),
                do_meta: meta,
                buffer: VecDeque::new(),
                dir_cache: self.dir_cache_builder(path2),
                iterator: Some(iter),
                fut: None,
            };
            Ok(Box::pin(strm) as FsStream<Box<dyn DavDirEntry>>)
        }
        .boxed()
    }

    fn open<'a>(
        &'a self,
        path: &'a DavPath,
        options: OpenOptions,
    ) -> FsFuture<'a, Box<dyn DavFile>> {
        async move {
            trace!("FS: open {:?}", self.fspath_dbg(path));
            if self.is_forbidden(path) {
                return Err(FsError::Forbidden);
            }
            #[cfg(unix)]
            let mode = if self.inner.public { 0o666 } else { 0o600 };
            let path = self.fspath(path)?;
            let mutating = options.write
                || options.append
                || options.truncate
                || options.create
                || options.create_new;
            let this = self.clone();
            self.blocking(move || {
                let path = if mutating {
                    let path = this.confine_leaf(&path)?;
                    LocalFs::reject_symlink(&path)?;
                    path
                } else {
                    this.confine_follow(&path)?
                };
                #[cfg(unix)]
                let res = {
                    let mut opts = std::fs::OpenOptions::new();
                    opts.read(options.read)
                        .write(options.write)
                        .append(options.append)
                        .truncate(options.truncate)
                        .create(options.create)
                        .create_new(options.create_new)
                        .mode(mode);
                    if mutating {
                        opts.custom_flags(libc::O_NOFOLLOW);
                    }
                    opts.open(path)
                };
                #[cfg(windows)]
                let res = std::fs::OpenOptions::new()
                    .read(options.read)
                    .write(options.write)
                    .append(options.append)
                    .truncate(options.truncate)
                    .create(options.create)
                    .create_new(options.create_new)
                    .open(path);
                match res {
                    Ok(file) => Ok(Box::new(LocalFsFile {
                        file: Some(file),
                        buf: BytesMut::new(),
                    }) as Box<dyn DavFile>),
                    Err(e) => Err(e.into()),
                }
            })
            .await
        }
        .boxed()
    }

    fn create_dir<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, ()> {
        async move {
            trace!("FS: create_dir {:?}", self.fspath_dbg(path));
            if self.is_forbidden(path) {
                return Err(FsError::Forbidden);
            }
            #[cfg(unix)]
            let mode = if self.inner.public { 0o777 } else { 0o700 };
            let path = self.fspath(path)?;
            let this = self.clone();
            self.blocking(move || {
                let path = this.confine_leaf(&path)?;
                LocalFs::reject_symlink(&path)?;
                #[cfg(unix)]
                {
                    std::fs::DirBuilder::new()
                        .mode(mode)
                        .create(path)
                        .map_err(|e| e.into())
                }
                #[cfg(windows)]
                {
                    std::fs::DirBuilder::new()
                        .create(path)
                        .map_err(|e| e.into())
                }
            })
            .await
        }
        .boxed()
    }

    #[cfg(feature = "caldav")]
    fn mark_calendar<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, ()> {
        self.mark_sidecar(path, ".dav-calendar")
    }

    #[cfg(feature = "carddav")]
    fn mark_addressbook<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, ()> {
        self.mark_sidecar(path, ".dav-addressbook")
    }

    fn remove_dir<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, ()> {
        async move {
            trace!("FS: remove_dir {:?}", self.fspath_dbg(path));
            let path = self.fspath(path)?;
            let this = self.clone();
            self.blocking(move || {
                let path = this.confine_leaf(&path)?;
                std::fs::remove_dir(path).map_err(|e| e.into())
            })
            .await
        }
        .boxed()
    }

    fn remove_file<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, ()> {
        async move {
            trace!("FS: remove_file {:?}", self.fspath_dbg(path));
            if self.is_forbidden(path) {
                return Err(FsError::Forbidden);
            }
            let path = self.fspath(path)?;
            let this = self.clone();
            self.blocking(move || {
                let path = this.confine_leaf(&path)?;
                std::fs::remove_file(path).map_err(|e| e.into())
            })
            .await
        }
        .boxed()
    }

    fn rename<'a>(&'a self, from: &'a DavPath, to: &'a DavPath) -> FsFuture<'a, ()> {
        async move {
            trace!(
                "FS: rename {:?} {:?}",
                self.fspath_dbg(from),
                self.fspath_dbg(to)
            );
            if self.is_forbidden(from) || self.is_forbidden(to) {
                return Err(FsError::Forbidden);
            }
            let frompath = self.fspath(from)?;
            let topath = self.fspath(to)?;
            let this = self.clone();
            self.blocking(move || {
                let frompath = this.confine_leaf(&frompath)?;
                let topath = this.confine_leaf(&topath)?;
                match std::fs::rename(&frompath, &topath) {
                    Ok(v) => Ok(v),
                    Err(e) => {
                        // webdav allows a rename from a directory to a file.
                        // note that this check is racy, and I'm not quite sure what
                        // we should do if the source is a symlink. anyway ...
                        if e.raw_os_error() == Some(libc::ENOTDIR) && frompath.is_dir() {
                            // remove and try again.
                            let _ = std::fs::remove_file(&topath);
                            std::fs::rename(frompath, topath).map_err(|e| e.into())
                        } else {
                            Err(e.into())
                        }
                    }
                }
            })
            .await
        }
        .boxed()
    }

    fn copy<'a>(&'a self, from: &'a DavPath, to: &'a DavPath) -> FsFuture<'a, ()> {
        async move {
            trace!(
                "FS: copy {:?} {:?}",
                self.fspath_dbg(from),
                self.fspath_dbg(to)
            );
            if self.is_forbidden(from) || self.is_forbidden(to) {
                return Err(FsError::Forbidden);
            }
            let path_from = self.fspath(from)?;
            let path_to = self.fspath(to)?;
            let this = self.clone();

            match self
                .blocking(move || {
                    let path_from = this.confine_follow(&path_from)?;
                    let path_to = this.confine_leaf(&path_to)?;
                    LocalFs::reject_symlink(&path_to)?;
                    reflink_or_copy(path_from, path_to).map_err(FsError::from)
                })
                .await
            {
                Ok(_) => Ok(()),
                Err(e) => {
                    debug!(
                        "copy({:?}, {:?}) failed: {}",
                        self.fspath_dbg(from),
                        self.fspath_dbg(to),
                        e
                    );
                    Err(e)
                }
            }
        }
        .boxed()
    }

    fn set_modified<'a>(&'a self, davpath: &'a DavPath, tm: SystemTime) -> FsFuture<'a, ()> {
        async move {
            trace!("FS: set_modified {:?}", self.fspath_dbg(davpath));
            if self.is_forbidden(davpath) {
                return Err(FsError::Forbidden);
            }
            let path = self.fspath(davpath)?;
            let this = self.clone();
            self.blocking(move || {
                let path = this.confine_leaf(&path)?;
                LocalFs::reject_symlink(&path)?;
                open_for_times(&path)?
                    .set_modified(tm)
                    .map_err(FsError::from)
            })
            .await
        }
        .boxed()
    }

    fn set_created<'a>(&'a self, davpath: &'a DavPath, tm: SystemTime) -> FsFuture<'a, ()> {
        async move {
            #[cfg(not(any(windows, target_vendor = "apple", target_os = "freebsd")))]
            {
                let _ = (self, davpath, tm);
                return Err(FsError::NotImplemented);
            }
            #[cfg(any(windows, target_vendor = "apple", target_os = "freebsd"))]
            {
                trace!("FS: set_created {:?}", self.fspath_dbg(davpath));
                if self.is_forbidden(davpath) {
                    return Err(FsError::Forbidden);
                }
                let path = self.fspath(davpath)?;
                let this = self.clone();
                self.blocking(move || {
                    let path = this.confine_leaf(&path)?;
                    LocalFs::reject_symlink(&path)?;
                    let file = open_for_times(&path)?;
                    let times = std::fs::FileTimes::new().set_created(tm);
                    file.set_times(times).map_err(FsError::from)
                })
                .await
            }
        }
        .boxed()
    }
}

// read_batch() result.
struct ReadDirBatch {
    iterator: Option<std::fs::ReadDir>,
    buffer: VecDeque<io::Result<LocalFsDirEntry>>,
}

// Read the next batch of LocalFsDirEntry structs (up to 256).
// This is sync code, must be run in `blocking()`.
fn read_batch(
    iterator: Option<std::fs::ReadDir>,
    fs: LocalFs,
    do_meta: ReadDirMeta,
) -> ReadDirBatch {
    let mut buffer = VecDeque::new();
    let mut iterator = match iterator {
        Some(i) => i,
        None => {
            return ReadDirBatch {
                buffer,
                iterator: None,
            };
        }
    };
    let _guard = match do_meta {
        ReadDirMeta::None => None,
        _ => fs.inner.fs_access_guard.as_ref().map(|f| f()),
    };
    for _ in 0..256 {
        match iterator.next() {
            Some(Ok(entry)) => {
                let meta = match do_meta {
                    ReadDirMeta::Data => Meta::Data(std::fs::metadata(entry.path())),
                    ReadDirMeta::DataSymlink => Meta::Data(entry.metadata()),
                    ReadDirMeta::None => Meta::Fs(fs.clone()),
                };
                let d = LocalFsDirEntry { meta, entry };
                buffer.push_back(Ok(d))
            }
            Some(Err(e)) => {
                buffer.push_back(Err(e));
                break;
            }
            None => break,
        }
    }
    ReadDirBatch {
        buffer,
        iterator: Some(iterator),
    }
}

impl LocalFsReadDir {
    // Create a future that calls read_batch().
    //
    // The 'iterator' is moved into the future, and returned when it completes,
    // together with a list of directory entries.
    fn read_batch(&mut self) -> BoxFuture<'static, ReadDirBatch> {
        let iterator = self.iterator.take();
        let fs = self.fs.clone();
        let do_meta = self.do_meta;

        let fut: BoxFuture<ReadDirBatch> =
            blocking(move || read_batch(iterator, fs, do_meta)).boxed();
        fut
    }
}

// The stream implementation tries to be smart and batch I/O operations
impl Stream for LocalFsReadDir {
    type Item = FsResult<Box<dyn DavDirEntry>>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = Pin::into_inner(self);

        // If the buffer is empty, fill it.
        if this.buffer.is_empty() {
            // If we have no pending future, create one.
            if this.fut.is_none() {
                if this.iterator.is_none() {
                    return Poll::Ready(None);
                }
                this.fut = Some(this.read_batch());
            }

            // Poll the future.
            let mut fut = pin!(this.fut.as_mut().unwrap());
            match Pin::new(&mut fut).poll(cx) {
                Poll::Ready(batch) => {
                    this.fut.take();
                    if let Some(ref mut nb) = this.dir_cache {
                        batch.buffer.iter().for_each(|e| {
                            if let Ok(e) = e {
                                nb.add(e.entry.file_name());
                            }
                        });
                    }
                    this.buffer = batch.buffer;
                    this.iterator = batch.iterator;
                }
                Poll::Pending => return Poll::Pending,
            }
        }

        // we filled the buffer, now pop from the buffer.
        match this.buffer.pop_front() {
            Some(Ok(item)) => Poll::Ready(Some(Ok(Box::new(item)))),
            Some(Err(err)) => {
                // fuse the iterator.
                this.iterator.take();
                // Do not finish the cache: a partial listing must not be treated
                // as complete, or missing ._ files would 404 until expiry.
                // return error of stream.
                Poll::Ready(Some(Err(err.into())))
            }
            None => {
                // fuse the iterator.
                this.iterator.take();
                // finish the cache.
                if let Some(ref mut nb) = this.dir_cache {
                    nb.finish();
                }
                // return end-of-stream.
                Poll::Ready(None)
            }
        }
    }
}

enum Is {
    File,
    Dir,
    Symlink,
}

impl LocalFsDirEntry {
    async fn is_a(&self, is: Is) -> FsResult<bool> {
        match self.meta {
            Meta::Data(Ok(ref meta)) => Ok(match is {
                Is::File => meta.file_type().is_file(),
                Is::Dir => meta.file_type().is_dir(),
                Is::Symlink => meta.file_type().is_symlink(),
            }),
            Meta::Data(Err(ref e)) => Err(e.into()),
            Meta::Fs(ref fs) => {
                let fullpath = self.entry.path();
                let ft = fs
                    .blocking(move || std::fs::symlink_metadata(&fullpath))
                    .await?
                    .file_type();
                Ok(match is {
                    Is::File => ft.is_file(),
                    Is::Dir => ft.is_dir(),
                    Is::Symlink => ft.is_symlink(),
                })
            }
        }
    }
}

impl DavDirEntry for LocalFsDirEntry {
    fn metadata(&'_ self) -> FsFuture<'_, Box<dyn DavMetaData>> {
        match self.meta {
            Meta::Data(ref meta) => {
                let fullpath = self.entry.path();
                let m = match meta {
                    Ok(meta) => Ok(Box::new(LocalFsMetaData::new(&fullpath, meta.clone()))
                        as Box<dyn DavMetaData>),
                    Err(e) => Err(e.into()),
                };
                Box::pin(future::ready(m))
            }
            Meta::Fs(ref fs) => {
                let fullpath = self.entry.path();
                fs.blocking(move || match std::fs::symlink_metadata(&fullpath) {
                    Ok(meta) => {
                        Ok(Box::new(LocalFsMetaData::new(&fullpath, meta)) as Box<dyn DavMetaData>)
                    }
                    Err(e) => Err(e.into()),
                })
                .boxed()
            }
        }
    }

    #[cfg(unix)]
    fn name(&self) -> Vec<u8> {
        self.entry.file_name().as_bytes().to_vec()
    }

    #[cfg(windows)]
    fn name(&self) -> Vec<u8> {
        self.entry
            .file_name()
            .to_string_lossy()
            .into_owned()
            .into_bytes()
    }

    fn is_dir(&'_ self) -> FsFuture<'_, bool> {
        Box::pin(self.is_a(Is::Dir))
    }

    fn is_file(&'_ self) -> FsFuture<'_, bool> {
        Box::pin(self.is_a(Is::File))
    }

    fn is_symlink(&'_ self) -> FsFuture<'_, bool> {
        Box::pin(self.is_a(Is::Symlink))
    }
}

/// Read into spare capacity so `buf.len()` never covers uninitialized bytes.
fn read_bytes_into(file: &mut impl Read, buf: &mut BytesMut, count: usize) -> io::Result<Bytes> {
    buf.reserve(count);
    let spare = buf.spare_capacity_mut();
    let to_read = count.min(spare.len());
    let n = {
        // SAFETY: `Read::read` initializes the prefix it reports as written.
        let dst =
            unsafe { std::slice::from_raw_parts_mut(spare.as_mut_ptr().cast::<u8>(), to_read) };
        file.read(dst)?
    };
    // SAFETY: `n` bytes of spare capacity were initialized by `read`.
    unsafe {
        buf.set_len(buf.len() + n);
    }
    Ok(buf.split().freeze())
}

impl DavFile for LocalFsFile {
    fn metadata(&'_ mut self) -> FsFuture<'_, Box<dyn DavMetaData>> {
        async move {
            let file = self.file.take().unwrap();
            let (meta, file) = blocking(move || (file.metadata(), file)).await;
            self.file = Some(file);
            Ok(Box::new(LocalFsMetaData::from_file_meta(meta?)) as Box<dyn DavMetaData>)
        }
        .boxed()
    }

    fn write_bytes(&'_ mut self, buf: Bytes) -> FsFuture<'_, ()> {
        async move {
            let mut file = self.file.take().unwrap();
            let (res, file) = blocking(move || (file.write_all(&buf), file)).await;
            self.file = Some(file);
            res.map_err(|e| e.into())
        }
        .boxed()
    }

    fn write_buf(&'_ mut self, mut buf: Box<dyn Buf + Send>) -> FsFuture<'_, ()> {
        async move {
            let mut file = self.file.take().unwrap();
            let (res, file) = blocking(move || {
                while buf.remaining() > 0 {
                    let n = match file.write(buf.chunk()) {
                        Ok(n) => n,
                        Err(e) => return (Err(e), file),
                    };
                    buf.advance(n);
                }
                (Ok(()), file)
            })
            .await;
            self.file = Some(file);
            res.map_err(|e| e.into())
        }
        .boxed()
    }

    fn read_bytes(&'_ mut self, count: usize) -> FsFuture<'_, Bytes> {
        async move {
            let mut file = self.file.take().unwrap();
            let mut buf = mem::take(&mut self.buf);
            let (res, file, buf) = blocking(move || {
                let res = read_bytes_into(&mut file, &mut buf, count);
                (res, file, buf)
            })
            .await;
            self.file = Some(file);
            self.buf = buf;
            res.map_err(|e| e.into())
        }
        .boxed()
    }

    fn seek(&'_ mut self, pos: SeekFrom) -> FsFuture<'_, u64> {
        async move {
            let mut file = self.file.take().unwrap();
            let (res, file) = blocking(move || (file.seek(pos), file)).await;
            self.file = Some(file);
            res.map_err(|e| e.into())
        }
        .boxed()
    }

    fn flush(&'_ mut self) -> FsFuture<'_, ()> {
        async move {
            let mut file = self.file.take().unwrap();
            let (res, file) = blocking(move || (file.flush(), file)).await;
            self.file = Some(file);
            res.map_err(|e| e.into())
        }
        .boxed()
    }
}

/// Convert a Windows FILETIME (100-ns ticks since 1601-01-01) to SystemTime.
#[cfg(any(windows, test))]
fn filetime_to_systemtime(ticks: u64) -> SystemTime {
    const UNIX_EPOCH_FILETIME: u64 = 116444736000000000;
    UNIX_EPOCH
        + Duration::from_nanos(
            ticks
                .saturating_sub(UNIX_EPOCH_FILETIME)
                .saturating_mul(100),
        )
}

impl DavMetaData for LocalFsMetaData {
    fn len(&self) -> u64 {
        self.meta.len()
    }
    fn created(&self) -> FsResult<SystemTime> {
        self.meta.created().map_err(|e| e.into())
    }
    fn modified(&self) -> FsResult<SystemTime> {
        self.meta.modified().map_err(|e| e.into())
    }
    fn accessed(&self) -> FsResult<SystemTime> {
        self.meta.accessed().map_err(|e| e.into())
    }

    #[cfg(unix)]
    fn status_changed(&self) -> FsResult<SystemTime> {
        Ok(UNIX_EPOCH + Duration::new(self.meta.ctime() as u64, 0))
    }

    #[cfg(windows)]
    fn status_changed(&self) -> FsResult<SystemTime> {
        Ok(filetime_to_systemtime(self.meta.creation_time()))
    }

    fn is_dir(&self) -> bool {
        self.meta.is_dir()
    }
    fn is_file(&self) -> bool {
        self.meta.is_file()
    }
    fn is_symlink(&self) -> bool {
        self.meta.file_type().is_symlink()
    }
    #[cfg(feature = "caldav")]
    fn is_calendar(&self, _: &DavPath) -> bool {
        self.is_calendar
    }
    #[cfg(feature = "carddav")]
    fn is_addressbook(&self, _: &DavPath) -> bool {
        self.is_addressbook
    }

    #[cfg(unix)]
    fn executable(&self) -> FsResult<bool> {
        if self.meta.is_file() {
            return Ok((self.meta.permissions().mode() & 0o100) > 0);
        }
        Err(FsError::NotImplemented)
    }

    #[cfg(windows)]
    fn executable(&self) -> FsResult<bool> {
        // Windows filesystem does not have an executable flag; for regular files we
        // assume they are executable, and for non-files we match the Unix behavior
        // by reporting this as not implemented.
        if self.meta.is_file() {
            Ok(true)
        } else {
            Err(FsError::NotImplemented)
        }
    }

    // same as the default apache etag.
    #[cfg(unix)]
    fn etag(&self) -> Option<String> {
        let modified = self.meta.modified().ok()?;
        let t = modified.duration_since(UNIX_EPOCH).ok()?;
        let t = t.as_secs() * 1000000 + t.subsec_nanos() as u64 / 1000;
        if self.is_file() {
            Some(format!(
                "{:x}-{:x}-{:x}",
                self.meta.ino(),
                self.meta.len(),
                t
            ))
        } else {
            Some(format!("{:x}-{:x}", self.meta.ino(), t))
        }
    }

    // same as the default apache etag.
    #[cfg(windows)]
    fn etag(&self) -> Option<String> {
        let modified = self.meta.modified().ok()?;
        let t = modified.duration_since(UNIX_EPOCH).ok()?;
        let t = t.as_secs() * 1000000 + t.subsec_nanos() as u64 / 1000;
        if self.is_file() {
            Some(format!("{:x}-{:x}", self.meta.len(), t))
        } else {
            Some(format!("{:x}", t))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filetime_unix_epoch() {
        assert_eq!(filetime_to_systemtime(116444736000000000), UNIX_EPOCH);
    }

    #[test]
    fn filetime_before_unix_epoch_saturates() {
        assert_eq!(filetime_to_systemtime(0), UNIX_EPOCH);
        assert_eq!(filetime_to_systemtime(116444736000000000 - 1), UNIX_EPOCH);
    }

    #[test]
    fn filetime_scales_100ns_ticks() {
        // One FILETIME tick is 100 ns; 10_000_000 ticks is one second.
        assert_eq!(
            filetime_to_systemtime(116444736000000000 + 1),
            UNIX_EPOCH + Duration::from_nanos(100)
        );
        assert_eq!(
            filetime_to_systemtime(116444736000000000 + 10_000_000),
            UNIX_EPOCH + Duration::from_secs(1)
        );
    }

    #[test]
    fn readdir_error_does_not_finish_appledouble_cache() {
        let dir = std::env::temp_dir().join(format!(
            "dav-readdir-err-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let fs = LocalFs::new(&dir, false, false, true);
        let mut strm = LocalFsReadDir {
            fs: (*fs).clone(),
            do_meta: ReadDirMeta::None,
            buffer: {
                let mut q = VecDeque::new();
                q.push_back(Err(io::Error::other("readdir failed")));
                q
            },
            dir_cache: Some(DUCacheBuilder::start(dir.clone())),
            iterator: None,
            fut: None,
        };
        let waker = futures_util::task::noop_waker();
        let mut cx = Context::from_waker(&waker);
        let poll = Pin::new(&mut strm).poll_next(&mut cx);
        assert!(matches!(poll, Poll::Ready(Some(Err(_)))));
        assert!(!fs.is_notfound(&dir.join("._nope")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn readdir_finish_marks_appledouble_cache_complete() {
        let dir = std::env::temp_dir().join(format!(
            "dav-readdir-eof-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let fs = LocalFs::new(&dir, false, false, true);
        let mut cache = DUCacheBuilder::start(dir.clone());
        cache.finish();
        assert!(fs.is_notfound(&dir.join("._nope")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn readdir_none_reports_symlink() {
        use futures_util::StreamExt;

        let dir = std::env::temp_dir().join(format!(
            "dav-readdir-none-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("target"), b"data").unwrap();
        std::os::unix::fs::symlink(dir.join("target"), dir.join("link")).unwrap();

        let fs = LocalFs::new(&dir, false, false, false);
        let path = DavPath::new("/").unwrap();
        let mut strm = DavFileSystem::read_dir(&*fs, &path, ReadDirMeta::None)
            .await
            .unwrap();

        let mut found = false;
        while let Some(entry) = strm.next().await {
            let entry = entry.unwrap();
            if entry.name() == b"link" {
                assert!(entry.is_symlink().await.unwrap());
                assert!(entry.metadata().await.unwrap().is_symlink());
                found = true;
            }
        }
        assert!(found, "symlink directory entry not found");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn join_normal_relative_segments() {
        let base = PathBuf::from("share");
        let out = join_rel_path(&base, Path::new("foo/bar")).unwrap();
        assert_eq!(out, PathBuf::from("share").join("foo").join("bar"));

        let fs = LocalFs::new(&base, false, false, false);
        let path = DavPath::new("/foo/bar").unwrap();
        assert_eq!(
            fs.fspath(&path).unwrap(),
            PathBuf::from("share").join("foo").join("bar")
        );
    }

    #[test]
    fn join_rejects_parent_cur_and_root() {
        let base = PathBuf::from("share");
        assert_eq!(
            join_rel_path(&base, Path::new("foo/../etc")),
            Err(FsError::Forbidden)
        );
        assert_eq!(
            join_rel_path(&base, Path::new("./foo")),
            Err(FsError::Forbidden)
        );
        assert_eq!(
            join_rel_path(&base, Path::new("/etc/passwd")),
            Err(FsError::Forbidden)
        );
    }

    #[test]
    fn join_rejects_names_that_push_would_treat_as_absolute() {
        let base = PathBuf::from("share");
        assert_eq!(
            push_normal_component(&mut base.clone(), OsStr::new("..")),
            Err(FsError::Forbidden)
        );
        assert_eq!(
            push_normal_component(&mut base.clone(), OsStr::new(".")),
            Err(FsError::Forbidden)
        );
        assert_eq!(
            push_normal_component(&mut base.clone(), OsStr::new("/etc")),
            Err(FsError::Forbidden)
        );
    }

    #[cfg(windows)]
    #[test]
    fn join_rejects_windows_drive_and_unc() {
        let base = PathBuf::from(r"C:\share");
        assert_eq!(
            join_rel_path(&base, Path::new("C:/Windows/win.ini")),
            Err(FsError::Forbidden)
        );
        assert_eq!(
            join_rel_path(&base, Path::new(r"\Windows\win.ini")),
            Err(FsError::Forbidden)
        );
        assert_eq!(
            join_rel_path(&base, Path::new(r"\\server\share\file")),
            Err(FsError::Forbidden)
        );
        assert_eq!(
            push_normal_component(&mut base.clone(), OsStr::new("C:")),
            Err(FsError::Forbidden)
        );

        let fs = LocalFs::new(&base, false, false, false);
        let path = DavPath::new("/C:/Windows/win.ini").unwrap();
        assert_eq!(fs.fspath(&path), Err(FsError::Forbidden));
    }

    struct FailRead;

    impl Read for FailRead {
        fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("fail"))
        }
    }

    struct PartialRead(usize);

    impl Read for PartialRead {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let n = self.0.min(buf.len());
            buf[..n].fill(b'x');
            Ok(n)
        }
    }

    #[test]
    fn read_bytes_into_error_does_not_extend_len() {
        let mut buf = BytesMut::new();
        buf.reserve(32);
        let err = read_bytes_into(&mut FailRead, &mut buf, 16).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Other);
        assert_eq!(buf.len(), 0);
    }

    #[test]
    fn read_bytes_into_success_splits_initialized_bytes() {
        let mut buf = BytesMut::new();
        let bytes = read_bytes_into(&mut PartialRead(3), &mut buf, 16).unwrap();
        assert_eq!(&bytes[..], b"xxx");
        assert_eq!(buf.len(), 0);
    }
}
