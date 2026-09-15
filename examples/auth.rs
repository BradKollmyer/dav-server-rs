//! Listing-filter example — **not authentication**.
//!
//! This is **not** a production 401 handler. The Basic username selects a
//! directory listing filter (`dirs` / `files` / `all`); any password is accepted.
//! Copying this as real authentication would let every password through.
//!
//! Example URLs:
//! - dav://dirs:-@127.0.0.1:4918 — responds only with directories.
//! - dav://files:-@127.0.0.1:4918 — responds only with files.

use std::{
    convert::Infallible, fmt::Display, future::Future, net::SocketAddr, path::Path, pin::Pin,
    time::SystemTime,
};

use futures_util::{StreamExt, stream};
use http::{Request, Response, StatusCode};
use hyper::{body::Incoming, server::conn::http1, service::service_fn};
use hyper_util::rt::TokioIo;
use tokio::{net::TcpListener, task::spawn};

use dav_server::{
    DavHandler,
    body::Body,
    davpath::DavPath,
    fakels::FakeLs,
    fs::{
        DavDirEntry, DavFile, DavMetaData, DavProp, FsFuture, FsResult, FsStream,
        GuardedFileSystem, OpenOptions, ReadDirMeta,
    },
    localfs::LocalFs,
};

#[tokio::main]
async fn main() {
    env_logger::init();
    let dir = "/tmp";
    let addr: SocketAddr = ([127, 0, 0, 1], 4918).into();
    let fs = FilteredFs::new(dir);
    let dav_server = DavHandler::builder()
        .filesystem(Box::new(fs) as _)
        .locksystem(FakeLs::new())
        .autoindex(true)
        .hide_symlinks(true)
        .build_handler();
    let listener = TcpListener::bind(addr).await.unwrap();
    println!("Listening {addr}");
    loop {
        let (stream, _client_addr) = listener.accept().await.unwrap();
        let dav_server = dav_server.clone();
        let io = TokioIo::new(stream);
        spawn(async move {
            let service = service_fn(move |request| handle(request, dav_server.clone()));
            if let Err(err) = http1::Builder::new().serve_connection(io, service).await {
                eprintln!("Failed serving: {err:?}");
            }
        });
    }
}

async fn handle(
    request: Request<Incoming>,
    handler: DavHandler<Filter>,
) -> Result<Response<Body>, Infallible> {
    /// https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/WWW-Authenticate
    static AUTH_CHALLENGE: &str = "Basic realm=\"Specify the directory entries' filter \
        as a username: `dirs`, `files` or `all`; password — any string\"";
    let filter = match Filter::from_request(&request) {
        Ok(f) => f,
        Err(err) => {
            // 401 here means "send a filter username", not a failed login.
            // This is **not authentication**; any password is accepted.
            let response = Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .header("WWW-Authenticate", AUTH_CHALLENGE)
                .body(err.to_string().into())
                .expect("Auth error response must be built fine");
            return Ok(response);
        }
    };
    Ok(handler
        .handle_guarded(request, "/principals/users/www_user".to_string(), filter)
        .await)
}

#[derive(Clone)]
struct FilteredFs {
    inner: Box<LocalFs>,
}

impl FilteredFs {
    fn new(dir: impl AsRef<Path>) -> Self {
        Self {
            inner: LocalFs::new(dir, false, false, false),
        }
    }
}

impl GuardedFileSystem<Filter> for FilteredFs {
    fn open<'a>(
        &'a self,
        path: &'a DavPath,
        options: OpenOptions,
        _credentials: &'a Filter,
    ) -> FsFuture<'a, Box<dyn DavFile>> {
        self.inner.open(path, options, &())
    }

    fn read_dir<'a>(
        &'a self,
        path: &'a DavPath,
        meta: ReadDirMeta,
        filter: &'a Filter,
    ) -> FsFuture<'a, FsStream<Box<dyn DavDirEntry>>> {
        Box::pin(async move {
            let mut stream = self.inner.read_dir(path, meta, &()).await?;
            let mut entries = Vec::default();
            while let Some(entry) = stream.next().await {
                let entry = entry?;
                if filter.matches(entry.as_ref()).await? {
                    entries.push(Ok(entry));
                }
            }
            Ok(Box::pin(stream::iter(entries)) as _)
        })
    }

    fn metadata<'a>(
        &'a self,
        path: &'a DavPath,
        _credentials: &'a Filter,
    ) -> FsFuture<'a, Box<dyn DavMetaData>> {
        self.inner.metadata(path, &())
    }

    fn symlink_metadata<'a>(
        &'a self,
        path: &'a DavPath,
        _credentials: &'a Filter,
    ) -> FsFuture<'a, Box<dyn DavMetaData>> {
        self.inner.symlink_metadata(path, &())
    }

    // The filter only affects listings; everything else is forwarded as-is.
    // Without these, writes would be 501 and MKCALENDAR / MKADDRESSBOOK would
    // silently create plain collections.

    fn create_dir<'a>(&'a self, path: &'a DavPath, _credentials: &'a Filter) -> FsFuture<'a, ()> {
        self.inner.create_dir(path, &())
    }

    fn remove_dir<'a>(&'a self, path: &'a DavPath, _credentials: &'a Filter) -> FsFuture<'a, ()> {
        self.inner.remove_dir(path, &())
    }

    fn remove_file<'a>(&'a self, path: &'a DavPath, _credentials: &'a Filter) -> FsFuture<'a, ()> {
        self.inner.remove_file(path, &())
    }

    fn rename<'a>(
        &'a self,
        from: &'a DavPath,
        to: &'a DavPath,
        _credentials: &'a Filter,
    ) -> FsFuture<'a, ()> {
        self.inner.rename(from, to, &())
    }

    fn copy<'a>(
        &'a self,
        from: &'a DavPath,
        to: &'a DavPath,
        _credentials: &'a Filter,
    ) -> FsFuture<'a, ()> {
        self.inner.copy(from, to, &())
    }

    fn set_modified<'a>(
        &'a self,
        path: &'a DavPath,
        tm: SystemTime,
        _credentials: &'a Filter,
    ) -> FsFuture<'a, ()> {
        self.inner.set_modified(path, tm, &())
    }

    fn set_created<'a>(
        &'a self,
        path: &'a DavPath,
        tm: SystemTime,
        _credentials: &'a Filter,
    ) -> FsFuture<'a, ()> {
        self.inner.set_created(path, tm, &())
    }

    fn have_props<'a>(
        &'a self,
        path: &'a DavPath,
        _credentials: &'a Filter,
    ) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
        self.inner.have_props(path, &())
    }

    #[cfg(feature = "proppatch")]
    fn patch_props<'a>(
        &'a self,
        path: &'a DavPath,
        patch: Vec<(bool, DavProp)>,
        _credentials: &'a Filter,
    ) -> FsFuture<'a, Vec<(StatusCode, DavProp)>> {
        self.inner.patch_props(path, patch, &())
    }

    fn get_props<'a>(
        &'a self,
        path: &'a DavPath,
        do_content: bool,
        _credentials: &'a Filter,
    ) -> FsFuture<'a, Vec<DavProp>> {
        self.inner.get_props(path, do_content, &())
    }

    fn get_prop<'a>(
        &'a self,
        path: &'a DavPath,
        prop: DavProp,
        _credentials: &'a Filter,
    ) -> FsFuture<'a, Vec<u8>> {
        self.inner.get_prop(path, prop, &())
    }

    fn get_quota<'a>(&'a self, _credentials: &'a Filter) -> FsFuture<'a, (u64, Option<u64>)> {
        self.inner.get_quota(&())
    }

    #[cfg(feature = "caldav")]
    fn mark_calendar<'a>(
        &'a self,
        path: &'a DavPath,
        _credentials: &'a Filter,
    ) -> FsFuture<'a, ()> {
        self.inner.mark_calendar(path, &())
    }

    #[cfg(feature = "carddav")]
    fn mark_addressbook<'a>(
        &'a self,
        path: &'a DavPath,
        _credentials: &'a Filter,
    ) -> FsFuture<'a, ()> {
        self.inner.mark_addressbook(path, &())
    }
}

#[derive(Clone)]
enum Filter {
    All,
    Files,
    Dirs,
}

impl Filter {
    /// Parse the Basic username as a listing filter (`dirs` / `files` / `all`).
    ///
    /// This is **not authentication**. The password is ignored; any value is
    /// accepted. Username only selects which directory entries are returned.
    fn from_request(request: &Request<Incoming>) -> Result<Self, Box<dyn Display>> {
        use headers::{Authorization, HeaderMapExt, authorization::Basic};

        let auth = request
            .headers()
            .typed_get::<Authorization<Basic>>()
            .ok_or(Box::new("please auth") as _)?;
        match auth.username() {
            "all" => Ok(Filter::All),
            "files" => Ok(Filter::Files),
            "dirs" => Ok(Filter::Dirs),
            _ => Err(Box::new("unexpected filter value") as _),
        }
    }

    async fn matches(&self, entry: &dyn DavDirEntry) -> FsResult<bool> {
        if let Filter::All = self {
            return Ok(true);
        }
        Ok(entry.is_dir().await? == matches!(self, Filter::Dirs))
    }
}
