//! Simple in-memory filesystem.
//!
//! This implementation has state, so if you create a
//! new instance in a handler(), it will be empty every time.
//!
//! This means you have to create the instance once, using `MemFs::new`, store
//! it in your handler struct, and clone() it every time you pass
//! it to the DavHandler. As a MemFs struct is just a handle, cloning is cheap.
//!
//! Lookup that walks through a regular file is forbidden. `MOVE` will not
//! replace an existing collection, even an empty one. Partial writes update
//! mtime (and therefore ETag).
use std::collections::HashMap;
use std::io::{Error, ErrorKind, SeekFrom};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use bytes::{Buf, Bytes};
use futures_util::{
    StreamExt, future,
    future::{BoxFuture, FutureExt},
};
use http::StatusCode;

use crate::davpath::DavPath;
use crate::fs::*;
use crate::tree;

type Tree = tree::Tree<Vec<u8>, MemFsNode>;

fn lock_tree(tree: &Mutex<Tree>) -> std::sync::MutexGuard<'_, Tree> {
    tree.lock().unwrap_or_else(|e| e.into_inner())
}

/// Ephemeral in-memory filesystem.
#[derive(Debug)]
pub struct MemFs {
    tree: Arc<Mutex<Tree>>,
}

#[derive(Debug, Clone)]
enum MemFsNode {
    Dir(MemFsDirNode),
    File(MemFsFileNode),
}

#[derive(Debug, Clone)]
struct MemFsDirNode {
    props: HashMap<String, DavProp>,
    mtime: SystemTime,
    crtime: SystemTime,
    #[cfg(feature = "caldav")]
    is_calendar: bool,
    #[cfg(feature = "carddav")]
    is_addressbook: bool,
}

#[derive(Debug, Clone)]
struct MemFsFileNode {
    props: HashMap<String, DavProp>,
    mtime: SystemTime,
    crtime: SystemTime,
    data: Vec<u8>,
}

#[derive(Debug, Clone)]
struct MemFsDirEntry {
    mtime: SystemTime,
    crtime: SystemTime,
    is_dir: bool,
    name: Vec<u8>,
    size: u64,
    #[cfg(feature = "caldav")]
    is_calendar: bool,
    #[cfg(feature = "carddav")]
    is_addressbook: bool,
}

#[derive(Debug)]
struct MemFsFile {
    tree: Arc<Mutex<Tree>>,
    node_id: u64,
    pos: usize,
    append: bool,
}

impl MemFs {
    /// Create a new "memfs" filesystem.
    pub fn new() -> Box<MemFs> {
        let tree = Tree::new(MemFsNode::new_dir());
        Box::new(MemFs {
            tree: Arc::new(Mutex::new(tree)),
        })
    }

    fn do_open(
        &self,
        tree: &mut Tree,
        path: &[u8],
        options: OpenOptions,
    ) -> FsResult<Box<dyn DavFile>> {
        let node_id = match tree.lookup(path) {
            Ok(n) => {
                if options.create_new {
                    return Err(FsError::Exists);
                }
                n
            }
            Err(FsError::NotFound) => {
                if !options.create {
                    return Err(FsError::NotFound);
                }
                let parent_id = tree.lookup_parent(path)?;
                tree.add_child(parent_id, file_name(path), MemFsNode::new_file(), true)?
            }
            Err(e) => return Err(e),
        };
        let node = tree.get_node_mut(node_id).unwrap();
        if node.is_dir() {
            return Err(FsError::Forbidden);
        }
        if options.truncate {
            node.as_file_mut()?.data.clear();
            node.update_mtime(SystemTime::now());
        }
        Ok(Box::new(MemFsFile {
            tree: self.tree.clone(),
            node_id,
            pos: 0,
            append: options.append,
        }))
    }
}

impl Clone for MemFs {
    fn clone(&self) -> Self {
        MemFs {
            tree: Arc::clone(&self.tree),
        }
    }
}

impl DavFileSystem for MemFs {
    fn metadata<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, Box<dyn DavMetaData>> {
        async move {
            let tree = &*lock_tree(&self.tree);
            let node_id = tree.lookup(path.as_bytes())?;
            let meta = tree.get_node(node_id)?.as_dirent(path.as_bytes());
            Ok(Box::new(meta) as Box<dyn DavMetaData>)
        }
        .boxed()
    }

    fn symlink_metadata<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, Box<dyn DavMetaData>> {
        <Self as DavFileSystem>::metadata(self, path)
    }

    fn read_dir<'a>(
        &'a self,
        path: &'a DavPath,
        _meta: ReadDirMeta,
    ) -> FsFuture<'a, FsStream<Box<dyn DavDirEntry>>> {
        async move {
            let tree = &*lock_tree(&self.tree);
            let node_id = tree.lookup(path.as_bytes())?;
            if !tree.get_node(node_id)?.is_dir() {
                return Err(FsError::Forbidden);
            }
            let mut v: Vec<Box<dyn DavDirEntry>> = Vec::new();
            for (name, dnode_id) in tree.get_children(node_id)? {
                if let Ok(node) = tree.get_node(dnode_id) {
                    v.push(Box::new(node.as_dirent(&name)));
                }
            }
            let strm = futures_util::stream::iter(v).map(Ok);
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
            let tree = &mut *lock_tree(&self.tree);
            self.do_open(tree, path.as_bytes(), options)
        }
        .boxed()
    }

    fn create_dir<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, ()> {
        async move {
            trace!("FS: create_dir {path:?}");
            let tree = &mut *lock_tree(&self.tree);
            let path = path.as_bytes();
            let parent_id = tree.lookup_parent(path)?;
            tree.add_child(parent_id, file_name(path), MemFsNode::new_dir(), false)?;
            tree.get_node_mut(parent_id)?
                .update_mtime(SystemTime::now());
            Ok(())
        }
        .boxed()
    }

    fn remove_file<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, ()> {
        async move {
            let tree = &mut *lock_tree(&self.tree);
            let parent_id = tree.lookup_parent(path.as_bytes())?;
            let node_id = tree.lookup(path.as_bytes())?;
            tree.delete_node(node_id)?;
            tree.get_node_mut(parent_id)?
                .update_mtime(SystemTime::now());
            Ok(())
        }
        .boxed()
    }

    fn remove_dir<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, ()> {
        async move {
            let tree = &mut *lock_tree(&self.tree);
            let parent_id = tree.lookup_parent(path.as_bytes())?;
            let node_id = tree.lookup(path.as_bytes())?;
            tree.delete_node(node_id)?;
            tree.get_node_mut(parent_id)?
                .update_mtime(SystemTime::now());
            Ok(())
        }
        .boxed()
    }

    fn rename<'a>(&'a self, from: &'a DavPath, to: &'a DavPath) -> FsFuture<'a, ()> {
        async move {
            let tree = &mut *lock_tree(&self.tree);
            let node_id = tree.lookup(from.as_bytes())?;
            let parent_id = tree.lookup_parent(from.as_bytes())?;
            let dst_id = tree.lookup_parent(to.as_bytes())?;
            if let Ok(existing) = tree.lookup(to.as_bytes())
                && tree.get_node(existing)?.is_dir()
            {
                return Err(FsError::Exists);
            }
            tree.move_node(node_id, dst_id, file_name(to.as_bytes()), true)?;
            tree.get_node_mut(parent_id)?
                .update_mtime(SystemTime::now());
            tree.get_node_mut(dst_id)?.update_mtime(SystemTime::now());
            Ok(())
        }
        .boxed()
    }

    fn copy<'a>(&'a self, from: &'a DavPath, to: &'a DavPath) -> FsFuture<'a, ()> {
        async move {
            let tree = &mut *lock_tree(&self.tree);

            // source must exist and be a file; collections are copied by the handler.
            let snode_id = tree.lookup(from.as_bytes())?;
            if tree.get_node(snode_id)?.is_dir() {
                return Err(FsError::Forbidden);
            }

            // make sure destination exists, create if needed.
            {
                let mut oo = OpenOptions::write();
                oo.create = true;
                self.do_open(tree, to.as_bytes(), oo)?;
            }
            let dnode_id = tree.lookup(to.as_bytes())?;

            let src = tree.get_node(snode_id)?.as_file()?.clone();
            let dest = tree.get_node_mut(dnode_id)?.as_file_mut()?;
            dest.props = src.props;
            dest.data = src.data;
            dest.mtime = SystemTime::now();

            Ok(())
        }
        .boxed()
    }

    fn set_modified<'a>(&'a self, path: &'a DavPath, tm: SystemTime) -> FsFuture<'a, ()> {
        async move {
            let tree = &mut *lock_tree(&self.tree);
            let node_id = tree.lookup(path.as_bytes())?;
            tree.get_node_mut(node_id)?.update_mtime(tm);
            Ok(())
        }
        .boxed()
    }

    fn set_created<'a>(&'a self, path: &'a DavPath, tm: SystemTime) -> FsFuture<'a, ()> {
        async move {
            let tree = &mut *lock_tree(&self.tree);
            let node_id = tree.lookup(path.as_bytes())?;
            tree.get_node_mut(node_id)?.update_crtime(tm);
            Ok(())
        }
        .boxed()
    }

    fn have_props<'a>(&'a self, _path: &'a DavPath) -> BoxFuture<'a, bool> {
        future::ready(true).boxed()
    }

    #[cfg(feature = "proppatch")]
    fn patch_props<'a>(
        &'a self,
        path: &'a DavPath,
        mut patch: Vec<(bool, DavProp)>,
    ) -> FsFuture<'a, Vec<(StatusCode, DavProp)>> {
        async move {
            let tree = &mut *lock_tree(&self.tree);
            let node_id = tree.lookup(path.as_bytes())?;
            let node = tree.get_node_mut(node_id)?;
            let props = node.get_props_mut();

            let mut res = Vec::new();

            for (set, p) in patch.drain(..) {
                let prop = cloneprop(&p);
                let status = if set {
                    props.insert(propkey(&p.namespace, &p.name), p);
                    StatusCode::OK
                } else {
                    props.remove(&propkey(&p.namespace, &p.name));
                    // the below map was added to signify if the remove succeeded or
                    // failed. however it seems that removing non-existant properties
                    // always succeed, so just return success.
                    //  .map(|_| StatusCode::OK).unwrap_or(StatusCode::NOT_FOUND)
                    StatusCode::OK
                };
                res.push((status, prop));
            }
            Ok(res)
        }
        .boxed()
    }

    fn get_props<'a>(&'a self, path: &'a DavPath, do_content: bool) -> FsFuture<'a, Vec<DavProp>> {
        async move {
            let tree = &mut *lock_tree(&self.tree);
            let node_id = tree.lookup(path.as_bytes())?;
            let node = tree.get_node(node_id)?;
            let mut res = Vec::new();
            for p in node.get_props().values() {
                res.push(if do_content { p.clone() } else { cloneprop(p) });
            }
            Ok(res)
        }
        .boxed()
    }

    fn get_prop<'a>(&'a self, path: &'a DavPath, prop: DavProp) -> FsFuture<'a, Vec<u8>> {
        async move {
            let tree = &mut *lock_tree(&self.tree);
            let node_id = tree.lookup(path.as_bytes())?;
            let node = tree.get_node(node_id)?;
            let p = node
                .get_props()
                .get(&propkey(&prop.namespace, &prop.name))
                .ok_or(FsError::NotFound)?;
            p.xml.clone().ok_or(FsError::NotFound)
        }
        .boxed()
    }

    #[cfg(feature = "caldav")]
    fn mark_calendar<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, ()> {
        async move {
            let tree = &mut *lock_tree(&self.tree);
            let node_id = tree.lookup(path.as_bytes())?;
            match tree.get_node_mut(node_id)? {
                MemFsNode::Dir(dir) => {
                    dir.is_calendar = true;
                    Ok(())
                }
                _ => Err(FsError::Forbidden),
            }
        }
        .boxed()
    }

    #[cfg(feature = "carddav")]
    fn mark_addressbook<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, ()> {
        async move {
            let tree = &mut *lock_tree(&self.tree);
            let node_id = tree.lookup(path.as_bytes())?;
            match tree.get_node_mut(node_id)? {
                MemFsNode::Dir(dir) => {
                    dir.is_addressbook = true;
                    Ok(())
                }
                _ => Err(FsError::Forbidden),
            }
        }
        .boxed()
    }
}

// small helper.
fn propkey(ns: &Option<String>, name: &str) -> String {
    ns.to_owned().as_ref().unwrap_or(&"".to_string()).clone() + name
}

// small helper.
fn cloneprop(p: &DavProp) -> DavProp {
    DavProp {
        name: p.name.clone(),
        namespace: p.namespace.clone(),
        prefix: p.prefix.clone(),
        xml: None,
    }
}

impl DavDirEntry for MemFsDirEntry {
    fn metadata(&'_ self) -> FsFuture<'_, Box<dyn DavMetaData>> {
        let meta = (*self).clone();
        Box::pin(future::ok(Box::new(meta) as Box<dyn DavMetaData>))
    }

    fn name(&self) -> Vec<u8> {
        self.name.clone()
    }
}

/// Grow `data` to `end` bytes, zero-filled, without aborting the process
/// when the allocation fails.
fn grow_to(data: &mut Vec<u8>, end: usize) -> FsResult<()> {
    if end > data.len() {
        data.try_reserve_exact(end - data.len())
            .map_err(|_| FsError::InsufficientStorage)?;
        data.resize(end, 0);
    }
    Ok(())
}

impl DavFile for MemFsFile {
    fn metadata(&'_ mut self) -> FsFuture<'_, Box<dyn DavMetaData>> {
        async move {
            let tree = &*lock_tree(&self.tree);
            let node = tree.get_node(self.node_id)?;
            let meta = node.as_dirent(b"");
            Ok(Box::new(meta) as Box<dyn DavMetaData>)
        }
        .boxed()
    }

    fn read_bytes(&'_ mut self, count: usize) -> FsFuture<'_, Bytes> {
        async move {
            let tree = &*lock_tree(&self.tree);
            let node = tree.get_node(self.node_id)?;
            let file = node.as_file()?;
            let curlen = file.data.len();
            let mut start = self.pos;
            let mut end = self.pos.saturating_add(count);
            if start > curlen {
                start = curlen
            }
            if end > curlen {
                end = curlen
            }
            let cnt = end - start;
            self.pos += cnt;
            Ok(Bytes::copy_from_slice(&file.data[start..end]))
        }
        .boxed()
    }

    fn write_bytes(&'_ mut self, buf: Bytes) -> FsFuture<'_, ()> {
        async move {
            let tree = &mut *lock_tree(&self.tree);
            let node = tree.get_node_mut(self.node_id)?;
            let file = node.as_file_mut()?;
            if self.append {
                self.pos = file.data.len();
            }
            let end = self.pos.checked_add(buf.len()).ok_or(FsError::TooLarge)?;
            grow_to(&mut file.data, end)?;
            file.data[self.pos..end].copy_from_slice(&buf);
            self.pos = end;
            file.mtime = SystemTime::now();
            Ok(())
        }
        .boxed()
    }

    fn write_buf(&'_ mut self, mut buf: Box<dyn Buf + Send>) -> FsFuture<'_, ()> {
        async move {
            let tree = &mut *lock_tree(&self.tree);
            let node = tree.get_node_mut(self.node_id)?;
            let file = node.as_file_mut()?;
            if self.append {
                self.pos = file.data.len();
            }
            let end = self
                .pos
                .checked_add(buf.remaining())
                .ok_or(FsError::TooLarge)?;
            grow_to(&mut file.data, end)?;
            while buf.has_remaining() {
                let b = buf.chunk();
                let len = b.len();
                file.data[self.pos..self.pos + len].copy_from_slice(b);
                buf.advance(len);
                self.pos += len;
            }
            file.mtime = SystemTime::now();
            Ok(())
        }
        .boxed()
    }

    fn flush(&'_ mut self) -> FsFuture<'_, ()> {
        future::ok(()).boxed()
    }

    fn seek(&'_ mut self, pos: SeekFrom) -> FsFuture<'_, u64> {
        async move {
            let invalid_seek =
                || FsError::from(Error::new(ErrorKind::InvalidInput, "invalid seek"));
            let (start, offset): (u64, i64) = match pos {
                SeekFrom::Start(npos) => {
                    self.pos = usize::try_from(npos).map_err(|_| invalid_seek())?;
                    return Ok(npos);
                }
                SeekFrom::Current(npos) => (self.pos as u64, npos),
                SeekFrom::End(npos) => {
                    let tree = &*lock_tree(&self.tree);
                    let node = tree.get_node(self.node_id)?;
                    let curlen = node.as_file()?.data.len() as u64;
                    (curlen, npos)
                }
            };
            let npos = if offset < 0 {
                start.checked_sub(offset.unsigned_abs())
            } else {
                start.checked_add(offset as u64)
            }
            .ok_or_else(invalid_seek)?;
            self.pos = usize::try_from(npos).map_err(|_| invalid_seek())?;
            Ok(npos)
        }
        .boxed()
    }
}

impl DavMetaData for MemFsDirEntry {
    fn len(&self) -> u64 {
        self.size
    }

    fn created(&self) -> FsResult<SystemTime> {
        Ok(self.crtime)
    }

    fn modified(&self) -> FsResult<SystemTime> {
        Ok(self.mtime)
    }

    fn is_dir(&self) -> bool {
        self.is_dir
    }

    fn is_symlink(&self) -> bool {
        false
    }

    #[cfg(feature = "caldav")]
    fn is_calendar(&self, _: &DavPath) -> bool {
        self.is_calendar
    }

    #[cfg(feature = "carddav")]
    fn is_addressbook(&self, _: &DavPath) -> bool {
        self.is_addressbook
    }
}

impl MemFsNode {
    fn new_dir() -> MemFsNode {
        MemFsNode::Dir(MemFsDirNode {
            crtime: SystemTime::now(),
            mtime: SystemTime::now(),
            props: HashMap::new(),
            #[cfg(feature = "caldav")]
            is_calendar: false,
            #[cfg(feature = "carddav")]
            is_addressbook: false,
        })
    }

    fn new_file() -> MemFsNode {
        MemFsNode::File(MemFsFileNode {
            crtime: SystemTime::now(),
            mtime: SystemTime::now(),
            props: HashMap::new(),
            data: Vec::new(),
        })
    }

    // helper to create MemFsDirEntry from a node.
    fn as_dirent(&self, name: &[u8]) -> MemFsDirEntry {
        match self {
            MemFsNode::File(file) => MemFsDirEntry {
                name: name.to_vec(),
                mtime: file.mtime,
                crtime: file.crtime,
                is_dir: false,
                size: file.data.len() as u64,
                #[cfg(feature = "caldav")]
                is_calendar: false,
                #[cfg(feature = "carddav")]
                is_addressbook: false,
            },
            MemFsNode::Dir(dir) => MemFsDirEntry {
                name: name.to_vec(),
                mtime: dir.mtime,
                crtime: dir.crtime,
                is_dir: true,
                size: 0,
                #[cfg(feature = "caldav")]
                is_calendar: dir.is_calendar,
                #[cfg(feature = "carddav")]
                is_addressbook: dir.is_addressbook,
            },
        }
    }

    fn update_mtime(&mut self, tm: std::time::SystemTime) {
        match *self {
            MemFsNode::Dir(ref mut d) => d.mtime = tm,
            MemFsNode::File(ref mut f) => f.mtime = tm,
        }
    }

    fn update_crtime(&mut self, tm: std::time::SystemTime) {
        match *self {
            MemFsNode::Dir(ref mut d) => d.crtime = tm,
            MemFsNode::File(ref mut f) => f.crtime = tm,
        }
    }

    fn is_dir(&self) -> bool {
        match *self {
            MemFsNode::Dir(_) => true,
            MemFsNode::File(_) => false,
        }
    }

    fn as_file(&self) -> FsResult<&MemFsFileNode> {
        match *self {
            MemFsNode::File(ref n) => Ok(n),
            _ => Err(FsError::Forbidden),
        }
    }

    fn as_file_mut(&mut self) -> FsResult<&mut MemFsFileNode> {
        match *self {
            MemFsNode::File(ref mut n) => Ok(n),
            _ => Err(FsError::Forbidden),
        }
    }

    fn get_props(&self) -> &HashMap<String, DavProp> {
        match *self {
            MemFsNode::File(ref n) => &n.props,
            MemFsNode::Dir(ref d) => &d.props,
        }
    }

    fn get_props_mut(&mut self) -> &mut HashMap<String, DavProp> {
        match *self {
            MemFsNode::File(ref mut n) => &mut n.props,
            MemFsNode::Dir(ref mut d) => &mut d.props,
        }
    }
}

trait TreeExt {
    fn lookup_segs(&self, segs: Vec<&[u8]>) -> FsResult<u64>;
    fn lookup(&self, path: &[u8]) -> FsResult<u64>;
    fn lookup_parent(&self, path: &[u8]) -> FsResult<u64>;
}

impl TreeExt for Tree {
    fn lookup_segs(&self, segs: Vec<&[u8]>) -> FsResult<u64> {
        let mut node_id = tree::ROOT_ID;
        for seg in segs.into_iter() {
            if !self.get_node(node_id)?.is_dir() {
                return Err(FsError::Forbidden);
            }
            node_id = self.get_child(node_id, seg)?;
        }
        Ok(node_id)
    }

    fn lookup(&self, path: &[u8]) -> FsResult<u64> {
        self.lookup_segs(
            path.split(|&c| c == b'/')
                .filter(|s| !s.is_empty())
                .collect(),
        )
    }

    // pop the last segment off the path, do a lookup, then
    // check if the result is a directory.
    fn lookup_parent(&self, path: &[u8]) -> FsResult<u64> {
        let mut segs: Vec<&[u8]> = path
            .split(|&c| c == b'/')
            .filter(|s| !s.is_empty())
            .collect();
        segs.pop();
        let node_id = self.lookup_segs(segs)?;
        if !self.get_node(node_id)?.is_dir() {
            return Err(FsError::Forbidden);
        }
        Ok(node_id)
    }
}

// helper
fn file_name(path: &[u8]) -> Vec<u8> {
    path.split(|&c| c == b'/')
        .rfind(|s| !s.is_empty())
        .unwrap_or(b"")
        .to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::davpath::DavPath;

    fn path(p: &str) -> DavPath {
        DavPath::new(p).unwrap()
    }

    async fn create_file(fs: &MemFs, p: &str, data: &[u8]) {
        let mut oo = OpenOptions::write();
        oo.create = true;
        oo.truncate = true;
        let mut f = DavFileSystem::open(fs, &path(p), oo).await.unwrap();
        if !data.is_empty() {
            f.write_bytes(Bytes::copy_from_slice(data)).await.unwrap();
        }
    }

    #[tokio::test]
    async fn write_past_usize_max_is_an_error_not_a_panic() {
        let fs = MemFs::new();
        create_file(&fs, "/file", b"hello").await;

        let mut f = DavFileSystem::open(&*fs, &path("/file"), OpenOptions::write())
            .await
            .unwrap();
        // Seeking to u64::MAX either fails outright (32-bit) or succeeds and
        // makes the following write overflow the position; neither may panic.
        let result = match f.seek(SeekFrom::Start(u64::MAX)).await {
            Ok(_) => f.write_bytes(Bytes::from_static(b"x")).await,
            Err(e) => Err(e),
        };
        assert!(result.is_err(), "{result:?}");

        let result = match f.seek(SeekFrom::Start(u64::MAX)).await {
            Ok(_) => f.write_buf(Box::new(Bytes::from_static(b"x"))).await,
            Err(e) => Err(e),
        };
        assert!(result.is_err(), "{result:?}");

        // Reading at a huge offset just returns nothing.
        if f.seek(SeekFrom::Start(u64::MAX)).await.is_ok() {
            assert!(f.read_bytes(16).await.unwrap().is_empty());
        }
        // Seeking outside the u64 range is rejected.
        if f.seek(SeekFrom::Start(u64::MAX)).await.is_ok() {
            assert!(f.seek(SeekFrom::Current(1)).await.is_err());
        }
        f.seek(SeekFrom::End(0)).await.unwrap();
        assert!(f.seek(SeekFrom::Current(-100)).await.is_err());

        // The file is untouched.
        f.seek(SeekFrom::Start(0)).await.unwrap();
        assert_eq!(&f.read_bytes(16).await.unwrap()[..], b"hello");
    }

    #[tokio::test]
    async fn lookup_through_file_is_forbidden() {
        let fs = MemFs::new();
        create_file(&fs, "/file", b"hello").await;

        let err = DavFileSystem::metadata(&*fs, &path("/file/anything"))
            .await
            .unwrap_err();
        assert!(
            matches!(err, FsError::Forbidden | FsError::NotFound),
            "{err:?}"
        );

        let meta = DavFileSystem::metadata(&*fs, &path("/file")).await.unwrap();
        assert!(!meta.is_dir());
        assert_eq!(meta.len(), 5);
    }

    #[tokio::test]
    async fn write_without_truncate_updates_mtime() {
        let fs = MemFs::new();
        create_file(&fs, "/file", b"hello").await;
        let p = path("/file");
        let old = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        DavFileSystem::set_modified(&*fs, &p, old).await.unwrap();

        let before = DavFileSystem::metadata(&*fs, &p).await.unwrap();
        let before_mtime = before.modified().unwrap();
        let before_etag = before.etag();
        assert_eq!(before_mtime, old);

        let mut oo = OpenOptions::write();
        oo.truncate = false;
        let mut f = DavFileSystem::open(&*fs, &p, oo).await.unwrap();
        f.write_bytes(Bytes::from_static(b"world")).await.unwrap();
        drop(f);

        let after = DavFileSystem::metadata(&*fs, &p).await.unwrap();
        assert_ne!(after.modified().unwrap(), before_mtime);
        assert_ne!(after.etag(), before_etag);
        assert_eq!(after.len(), 5);

        DavFileSystem::set_modified(&*fs, &p, old).await.unwrap();
        let mut oo = OpenOptions::write();
        oo.truncate = false;
        let mut f = DavFileSystem::open(&*fs, &p, oo).await.unwrap();
        f.write_buf(Box::new(Bytes::from_static(b"abcde")))
            .await
            .unwrap();
        drop(f);

        let after_buf = DavFileSystem::metadata(&*fs, &p).await.unwrap();
        assert_ne!(after_buf.modified().unwrap(), old);
        assert_ne!(after_buf.etag(), before_etag);
    }

    #[tokio::test]
    async fn rename_does_not_overwrite_empty_collection() {
        let fs = MemFs::new();
        DavFileSystem::create_dir(&*fs, &path("/dir"))
            .await
            .unwrap();
        create_file(&fs, "/file", b"hello").await;

        let err = DavFileSystem::rename(&*fs, &path("/file"), &path("/dir"))
            .await
            .unwrap_err();
        assert_eq!(err, FsError::Exists);

        let dir = DavFileSystem::metadata(&*fs, &path("/dir")).await.unwrap();
        assert!(dir.is_dir());
        let file = DavFileSystem::metadata(&*fs, &path("/file")).await.unwrap();
        assert!(!file.is_dir());
        assert_eq!(file.len(), 5);
    }

    #[tokio::test]
    async fn copy_of_directory_is_error() {
        let fs = MemFs::new();
        DavFileSystem::create_dir(&*fs, &path("/dir"))
            .await
            .unwrap();
        create_file(&fs, "/dir/child", b"x").await;

        let err = DavFileSystem::copy(&*fs, &path("/dir"), &path("/dest"))
            .await
            .unwrap_err();
        assert!(
            matches!(err, FsError::Forbidden | FsError::NotImplemented),
            "{err:?}"
        );

        let src = DavFileSystem::metadata(&*fs, &path("/dir")).await.unwrap();
        assert!(src.is_dir());
        assert!(DavFileSystem::metadata(&*fs, &path("/dest")).await.is_err());
    }

    #[tokio::test]
    async fn recovers_from_poisoned_mutex() {
        let fs = MemFs::new();
        let tree = fs.tree.clone();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = tree.lock().unwrap();
            panic!("poison");
        }));
        let meta = DavFileSystem::metadata(&*fs, &path("/")).await.unwrap();
        assert!(meta.is_dir());
    }
}
