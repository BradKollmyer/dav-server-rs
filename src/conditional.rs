use std::time::{Duration, SystemTime, UNIX_EPOCH};

use headers::HeaderMapExt;
use http::{Method, StatusCode};

use crate::davheaders::{self, ETag};
use crate::davpath::DavPath;
use crate::fs::{DavMetaData, GuardedFileSystem};
use crate::ls::{DavLock, DavLockSystem};

type Request = http::Request<()>;

// SystemTime has nanosecond precision. Round it down to the
// nearest second, because an HttpDate has second precision.
fn round_time(tm: impl Into<SystemTime>) -> SystemTime {
    let tm = tm.into();
    match tm.duration_since(UNIX_EPOCH) {
        Ok(d) => UNIX_EPOCH + Duration::from_secs(d.as_secs()),
        Err(_) => tm,
    }
}

pub(crate) fn ifrange_match(
    hdr: &davheaders::IfRange,
    tag: Option<&davheaders::ETag>,
    date: Option<SystemTime>,
) -> bool {
    match *hdr {
        davheaders::IfRange::Date(ref d) => match date {
            Some(date) => round_time(date) == round_time(*d),
            None => false,
        },
        davheaders::IfRange::ETag(ref t) => match tag {
            Some(tag) => t == tag,
            None => false,
        },
    }
}

pub(crate) fn etaglist_match(
    tags: &davheaders::ETagList,
    exists: bool,
    tag: Option<&davheaders::ETag>,
) -> bool {
    match *tags {
        davheaders::ETagList::Star => exists,
        davheaders::ETagList::Tags(ref t) => match tag {
            Some(tag) => t.iter().any(|x| x == tag),
            None => false,
        },
    }
}

// Handle the if-headers: RFC 7232, HTTP/1.1 Conditional Requests.
pub(crate) fn http_if_match(req: &Request, meta: Option<&dyn DavMetaData>) -> Option<StatusCode> {
    let file_modified = meta.and_then(|m| m.modified().ok());

    if let Some(r) = req.headers().typed_get::<davheaders::IfMatch>() {
        let etag = meta.and_then(ETag::from_meta);
        if !etaglist_match(&r.0, meta.is_some(), etag.as_ref()) {
            trace!("precondition fail: If-Match {r:?}");
            return Some(StatusCode::PRECONDITION_FAILED);
        }
    } else if let Some(r) = req.headers().typed_get::<headers::IfUnmodifiedSince>() {
        match file_modified {
            None => return Some(StatusCode::PRECONDITION_FAILED),
            Some(file_modified) => {
                if round_time(file_modified) > round_time(r) {
                    trace!("precondition fail: If-Unmodified-Since {r:?}");
                    return Some(StatusCode::PRECONDITION_FAILED);
                }
            }
        }
    }

    if let Some(r) = req.headers().typed_get::<davheaders::IfNoneMatch>() {
        let etag = meta.and_then(ETag::from_meta);
        if etaglist_match(&r.0, meta.is_some(), etag.as_ref()) {
            trace!("precondition fail: If-None-Match {r:?}");
            if req.method() == Method::GET || req.method() == Method::HEAD {
                return Some(StatusCode::NOT_MODIFIED);
            } else {
                return Some(StatusCode::PRECONDITION_FAILED);
            }
        }
    } else if let Some(r) = req.headers().typed_get::<headers::IfModifiedSince>()
        && (req.method() == Method::GET || req.method() == Method::HEAD)
        && let Some(file_modified) = file_modified
        && round_time(file_modified) <= round_time(r)
    {
        trace!("not-modified If-Modified-Since {r:?}");
        return Some(StatusCode::NOT_MODIFIED);
    }
    None
}

// Strip a trailing slash so /dir and /dir/ compare equal. Keep "/" as-is.
fn davpath_key(path: &DavPath) -> &[u8] {
    let b = path.as_bytes();
    if b.len() > 1 && b.ends_with(b"/") {
        &b[..b.len() - 1]
    } else {
        b
    }
}

// A token is true only if it identifies a lock that currently applies to the
// tagged resource (RFC4918 10.4.4). MemLs::discover also yields Depth:0
// ancestor locks, which do not cover descendants.
fn lock_covers_path(lock: &DavLock, path: &DavPath) -> bool {
    let lock_key = davpath_key(lock.path.as_ref());
    let path_key = davpath_key(path);
    if lock_key == path_key {
        return true;
    }
    if !lock.deep {
        return false;
    }
    if lock_key == b"/" {
        return true;
    }
    path_key.starts_with(lock_key) && path_key.get(lock_key.len()) == Some(&b'/')
}

// handle the If header: RFC4918, 10.4.  If Header
//
// returns true if the header was not present, or if any of the iflists
// evaluated to true. Also returns a Vec of StateTokens that we encountered.
//
// caller should set the http status to 412 PreconditionFailed if
// the return value from this function is false.
//
pub(crate) async fn dav_if_match<'a, C>(
    req: &'a Request,
    fs: &'a (dyn GuardedFileSystem<C> + 'static),
    ls: &'a Option<Box<dyn DavLockSystem + 'static>>,
    path: &'a DavPath,
    credentials: &C,
) -> (bool, Vec<String>)
where
    C: Clone + Send + Sync + 'static,
{
    let mut tokens: Vec<String> = Vec::new();
    let mut any_list_ok = false;

    let r = match req.headers().typed_get::<davheaders::If>() {
        Some(r) => r,
        None => return (true, tokens),
    };

    for iflist in r.0.iter() {
        // save and return all statetokens that we encountered.
        let toks = iflist.conditions.iter().filter_map(|c| match c.item {
            davheaders::IfItem::StateToken(ref t) => Some(t.to_owned()),
            _ => None,
        });
        tokens.extend(toks);

        // skip over if a previous list already evaluated to true.
        if any_list_ok {
            continue;
        }

        // find the resource that this list is about.
        let mut pa: Option<DavPath> = None;
        let (p, valid) = match iflist.resource_tag {
            Some(ref url) => {
                // Host mismatch is an invalid location (condition false), not 502.
                match DavPath::from_str_and_prefix(url.path(), path.prefix()) {
                    Ok(p) if davheaders::request_is_same_server(req, url) => {
                        // anchor davpath in pa.
                        let p: &DavPath = pa.get_or_insert(p);
                        (p, true)
                    }
                    _ => (path, false),
                }
            }
            None => (path, true),
        };

        // now process the conditions. they must all be true.
        let mut list_ok = false;
        for cond in iflist.conditions.iter() {
            let cond_ok = match cond.item {
                davheaders::IfItem::StateToken(ref s) => {
                    // tokens in DAV: namespace always evaluate to false (10.4.8)
                    if !valid || s.starts_with("DAV:") {
                        false
                    } else {
                        match *ls {
                            Some(ref ls) => ls
                                .discover(p)
                                .await
                                .iter()
                                .any(|lock| lock.token == *s && lock_covers_path(lock, p)),
                            None => false,
                        }
                    }
                }
                davheaders::IfItem::ETag(ref tag) => {
                    if !valid {
                        // invalid location, so always false.
                        false
                    } else {
                        match fs.metadata(p, credentials).await {
                            Ok(meta) => {
                                // exists and may have metadata ..
                                if let Some(mtag) = ETag::from_meta(meta.as_ref()) {
                                    tag == &mtag
                                } else {
                                    false
                                }
                            }
                            Err(_) => {
                                // metadata error, fail.
                                false
                            }
                        }
                    }
                }
            };
            if cond_ok == cond.not {
                list_ok = false;
                break;
            }
            list_ok = true;
        }
        if list_ok {
            any_list_ok = true;
        }
    }
    if !any_list_ok {
        trace!("precondition fail: If {:?}", r.0);
    }
    (any_list_ok, tokens)
}

// Handle both the HTTP conditional If: headers, and the webdav If: header.
pub(crate) async fn if_match<'a, C>(
    req: &'a Request,
    meta: Option<&'a (dyn DavMetaData + 'static)>,
    fs: &'a (dyn GuardedFileSystem<C> + 'static),
    ls: &'a Option<Box<dyn DavLockSystem + 'static>>,
    path: &'a DavPath,
    credentials: &C,
) -> Option<StatusCode>
where
    C: Clone + Send + Sync + 'static,
{
    match dav_if_match(req, fs, ls, path, credentials).await {
        (true, _) => {}
        (false, _) => return Some(StatusCode::PRECONDITION_FAILED),
    }
    http_if_match(req, meta)
}

// Like if_match, but also returns all "associated state-tokens"
pub(crate) async fn if_match_get_tokens<'a, C>(
    req: &'a Request,
    meta: Option<&'a (dyn DavMetaData + 'static)>,
    fs: &'a (dyn GuardedFileSystem<C> + 'static),
    ls: &'a Option<Box<dyn DavLockSystem + 'static>>,
    path: &'a DavPath,
    credentials: &C,
) -> Result<Vec<String>, StatusCode>
where
    C: Clone + Send + Sync + 'static,
{
    if let Some(code) = http_if_match(req, meta) {
        return Err(code);
    }
    match dav_if_match(req, fs, ls, path, credentials).await {
        (true, v) => Ok(v),
        (false, _) => Err(StatusCode::PRECONDITION_FAILED),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memls::MemLs;
    use crate::voidfs::VoidFs;

    fn req_if(uri: &str, if_header: &str) -> Request {
        http::Request::builder()
            .method("PUT")
            .uri(uri)
            .header("If", if_header)
            .body(())
            .unwrap()
    }

    #[tokio::test]
    async fn garbage_token_on_unlocked_resource_is_false() {
        let fs = VoidFs::<()>::new();
        let ls: Option<Box<dyn DavLockSystem>> = Some(MemLs::new());
        let path = DavPath::new("/file").unwrap();
        let req = req_if("/file", "(<opaquelocktoken:garbage>)");
        let (ok, tokens) = dav_if_match(&req, fs.as_ref(), &ls, &path, &()).await;
        assert!(!ok);
        assert_eq!(tokens, vec!["opaquelocktoken:garbage".to_string()]);
    }

    #[tokio::test]
    async fn not_garbage_token_on_unlocked_resource_is_true() {
        let fs = VoidFs::<()>::new();
        let ls: Option<Box<dyn DavLockSystem>> = Some(MemLs::new());
        let path = DavPath::new("/file").unwrap();
        let req = req_if("/file", "(Not <opaquelocktoken:garbage>)");
        let (ok, tokens) = dav_if_match(&req, fs.as_ref(), &ls, &path, &()).await;
        assert!(ok);
        assert_eq!(tokens, vec!["opaquelocktoken:garbage".to_string()]);
    }

    #[tokio::test]
    async fn live_lock_token_matches_locked_resource() {
        let fs = VoidFs::<()>::new();
        let ls: Option<Box<dyn DavLockSystem>> = Some(MemLs::new());
        let path = DavPath::new("/file").unwrap();
        let lock = ls
            .as_ref()
            .unwrap()
            .lock(&path, None, None, None, false, false)
            .await
            .unwrap();
        let req = req_if("/file", &format!("(<{}>)", lock.token));
        let (ok, tokens) = dav_if_match(&req, fs.as_ref(), &ls, &path, &()).await;
        assert!(ok);
        assert_eq!(tokens, vec![lock.token]);
    }

    #[tokio::test]
    async fn depth_zero_lock_does_not_cover_child() {
        let fs = VoidFs::<()>::new();
        let ls: Option<Box<dyn DavLockSystem>> = Some(MemLs::new());
        let root = DavPath::new("/").unwrap();
        let child = DavPath::new("/child").unwrap();
        let lock = ls
            .as_ref()
            .unwrap()
            .lock(&root, None, None, None, false, false)
            .await
            .unwrap();
        let req = req_if("/child", &format!("(<{}>)", lock.token));
        let (ok, _) = dav_if_match(&req, fs.as_ref(), &ls, &child, &()).await;
        assert!(!ok);
    }

    #[tokio::test]
    async fn deep_lock_covers_child() {
        let fs = VoidFs::<()>::new();
        let ls: Option<Box<dyn DavLockSystem>> = Some(MemLs::new());
        let root = DavPath::new("/").unwrap();
        let child = DavPath::new("/child").unwrap();
        let lock = ls
            .as_ref()
            .unwrap()
            .lock(&root, None, None, None, false, true)
            .await
            .unwrap();
        let req = req_if("/child", &format!("(<{}>)", lock.token));
        let (ok, _) = dav_if_match(&req, fs.as_ref(), &ls, &child, &()).await;
        assert!(ok);
    }

    #[tokio::test]
    async fn dav_namespace_token_is_false() {
        let fs = VoidFs::<()>::new();
        let ls: Option<Box<dyn DavLockSystem>> = Some(MemLs::new());
        let path = DavPath::new("/file").unwrap();
        let req = req_if("/file", "(<DAV:no-lock>)");
        let (ok, tokens) = dav_if_match(&req, fs.as_ref(), &ls, &path, &()).await;
        assert!(!ok);
        assert_eq!(tokens, vec!["DAV:no-lock".to_string()]);
    }

    #[tokio::test]
    async fn tagged_if_other_host_is_false() {
        let fs = VoidFs::<()>::new();
        let ls: Option<Box<dyn DavLockSystem>> = Some(MemLs::new());
        let path = DavPath::new("/file").unwrap();
        let req = req_if(
            "http://example.com/file",
            "<http://evil.example/file> (<opaquelocktoken:garbage>)",
        );
        let (ok, _) = dav_if_match(&req, fs.as_ref(), &ls, &path, &()).await;
        assert!(!ok);
    }

    #[tokio::test]
    async fn tagged_if_same_host_default_port_is_valid_location() {
        let fs = VoidFs::<()>::new();
        let ls: Option<Box<dyn DavLockSystem>> = Some(MemLs::new());
        let path = DavPath::new("/file").unwrap();
        let req = req_if(
            "http://example.com/file",
            "<http://example.com:80/file> (Not <DAV:no-lock>)",
        );
        let (ok, _) = dav_if_match(&req, fs.as_ref(), &ls, &path, &()).await;
        assert!(ok);
    }
}
