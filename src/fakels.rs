//! Fake locksystem (to make Windows/macOS work).
//!
//! Several Webdav clients, like the ones on Windows and macOS, require just
//! basic functionality to mount the Webdav server in read-only mode. However
//! to be able to mount the Webdav server in read-write mode, they require the
//! Webdav server to have Webdav class 2 compliance - that means, LOCK/UNLOCK
//! support.
//!
//! In many cases, this is not actually important. A lot of the current Webdav
//! server implementations that are used to serve a filesystem just fake it:
//! LOCK/UNLOCK always succeed, checking for locktokens in
//! If: headers always succeeds, and nothing is every really locked.
//!
//! `FakeLs` implements such a fake locksystem. It is **not** an authorization
//! boundary: `check` always succeeds, so leftover or missing tokens never
//! exclude another client. Issued locks are stored only so `discover` /
//! `PROPFIND` `lockdiscovery` can round-trip the token.
//!
//! This implementation has state. Create it once with [`FakeLs::new`], store
//! it in your handler struct, and clone it when passing it to the
//! `DavHandler`. Cloning is cheap (the struct is just a handle).
use std::collections::HashMap;
use std::future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use futures_util::FutureExt;
use uuid::Uuid;
use xmltree::Element;

use crate::davpath::DavPath;
use crate::ls::*;

/// Fake locksystem implementation.
///
/// Not an authorization boundary: [`DavLockSystem::check`] always succeeds.
#[derive(Debug, Clone)]
pub struct FakeLs(Arc<Mutex<HashMap<String, DavLock>>>);

impl FakeLs {
    /// Create a new "fakels" locksystem.
    pub fn new() -> Box<FakeLs> {
        Box::new(FakeLs(Arc::new(Mutex::new(HashMap::new()))))
    }
}

fn tm_limit(d: Option<Duration>) -> Duration {
    match d {
        None => Duration::new(120, 0),
        Some(d) => {
            if d.as_secs() > 120 {
                Duration::new(120, 0)
            } else {
                d
            }
        }
    }
}

// Same covering rule as If tokens in `conditional`: lock on the resource,
// or a Depth: infinity lock on an ancestor. Depth: 0 ancestor locks do not
// cover descendants.
fn davpath_key(path: &DavPath) -> &[u8] {
    let b = path.as_bytes();
    if b.len() > 1 && b.ends_with(b"/") {
        &b[..b.len() - 1]
    } else {
        b
    }
}

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

impl DavLockSystem for FakeLs {
    fn lock(
        &'_ self,
        path: &DavPath,
        principal: Option<&str>,
        owner: Option<&Element>,
        timeout: Option<Duration>,
        shared: bool,
        deep: bool,
    ) -> LsFuture<'_, Result<DavLock, DavLock>> {
        let timeout = tm_limit(timeout);
        let timeout_at = SystemTime::now() + timeout;

        let d = if deep { 'I' } else { '0' };
        let s = if shared { 'S' } else { 'E' };
        let token = format!("opaquetoken:{}/{}/{}", Uuid::new_v4().hyphenated(), d, s);

        let lock = DavLock {
            token,
            path: Box::new(path.clone()),
            principal: principal.map(|s| s.to_string()),
            owner: owner.map(|o| Box::new(o.clone())),
            timeout_at: Some(timeout_at),
            timeout: Some(timeout),
            shared,
            deep,
        };
        debug!("lock {} created", lock.token);
        self.0
            .lock()
            .unwrap()
            .insert(lock.token.clone(), lock.clone());
        future::ready(Ok(lock)).boxed()
    }

    fn unlock(&'_ self, _path: &DavPath, token: &str) -> LsFuture<'_, Result<(), ()>> {
        self.0.lock().unwrap().remove(token);
        future::ready(Ok(())).boxed()
    }

    fn refresh(
        &'_ self,
        path: &DavPath,
        token: &str,
        timeout: Option<Duration>,
    ) -> LsFuture<'_, Result<DavLock, ()>> {
        debug!("refresh lock {token}");
        let timeout = tm_limit(timeout);
        let timeout_at = SystemTime::now() + timeout;

        let mut locks = self.0.lock().unwrap();
        if let Some(lock) = locks.get_mut(token) {
            lock.timeout = Some(timeout);
            lock.timeout_at = Some(timeout_at);
            return future::ready(Ok(lock.clone())).boxed();
        }

        let v: Vec<&str> = token.split('/').collect();
        let deep = v.len() > 1 && v[1] == "I";
        let shared = v.len() > 2 && v[2] == "S";

        let lock = DavLock {
            token: token.to_string(),
            path: Box::new(path.clone()),
            principal: None,
            owner: None,
            timeout_at: Some(timeout_at),
            timeout: Some(timeout),
            shared,
            deep,
        };
        future::ready(Ok(lock)).boxed()
    }

    fn check(
        &'_ self,
        _path: &DavPath,
        _principal: Option<&str>,
        _ignore_principal: bool,
        _deep: bool,
        _submitted_tokens: &[String],
    ) -> LsFuture<'_, Result<(), DavLock>> {
        future::ready(Ok(())).boxed()
    }

    fn discover(&'_ self, path: &DavPath) -> LsFuture<'_, Vec<DavLock>> {
        let locks = self.0.lock().unwrap();
        let found = locks
            .values()
            .filter(|lock| lock_covers_path(lock, path))
            .cloned()
            .collect();
        future::ready(found).boxed()
    }

    fn delete(&'_ self, path: &DavPath) -> LsFuture<'_, Result<(), ()>> {
        let del_key = davpath_key(path).to_vec();
        self.0.lock().unwrap().retain(|_, lock| {
            let lock_key = davpath_key(lock.path.as_ref());
            if lock_key == del_key {
                return false;
            }
            if del_key == b"/" {
                return false;
            }
            !(lock_key.starts_with(&del_key) && lock_key.get(del_key.len()) == Some(&b'/'))
        });
        future::ready(Ok(())).boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::davpath::DavPath;
    use futures_util::FutureExt;
    use std::time::Duration;

    fn ready<T>(fut: impl std::future::Future<Output = T>) -> T {
        fut.now_or_never()
            .expect("FakeLs futures complete immediately")
    }

    #[test]
    fn discover_includes_lock_on_request_path() {
        let ls = FakeLs::new();
        let child = DavPath::new("/child").unwrap();
        let lock = ready(ls.lock(&child, None, None, None, false, false)).unwrap();

        let found = ready(ls.discover(&child));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].token, lock.token);
    }

    #[test]
    fn discover_includes_deep_ancestor_lock() {
        let ls = FakeLs::new();
        let root = DavPath::new("/").unwrap();
        let child = DavPath::new("/child").unwrap();
        let lock = ready(ls.lock(&root, None, None, None, false, true)).unwrap();

        let found = ready(ls.discover(&child));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].token, lock.token);
    }

    #[test]
    fn discover_omits_depth_zero_ancestor_lock() {
        let ls = FakeLs::new();
        let root = DavPath::new("/").unwrap();
        let child = DavPath::new("/child").unwrap();
        let lock = ready(ls.lock(&root, None, None, None, false, false)).unwrap();

        let found = ready(ls.discover(&child));
        assert!(!found.iter().any(|l| l.token == lock.token));
    }

    #[test]
    fn unlock_removes_lock_from_discover() {
        let ls = FakeLs::new();
        let p = DavPath::new("/file").unwrap();
        let lock =
            ready(ls.lock(&p, None, None, Some(Duration::from_secs(60)), false, false)).unwrap();
        assert_eq!(ready(ls.unlock(&p, &lock.token)), Ok(()));
        assert!(ready(ls.discover(&p)).is_empty());
    }

    #[test]
    fn check_always_succeeds_while_lock_is_stored() {
        let ls = FakeLs::new();
        let p = DavPath::new("/file").unwrap();
        ready(ls.lock(&p, None, None, None, false, false)).unwrap();
        assert!(ready(ls.check(&p, None, true, false, &[])).is_ok());
    }

    #[test]
    fn refresh_updates_stored_lock() {
        let ls = FakeLs::new();
        let p = DavPath::new("/file").unwrap();
        let lock =
            ready(ls.lock(&p, None, None, Some(Duration::from_secs(30)), false, false)).unwrap();
        let refreshed = ready(ls.refresh(&p, &lock.token, Some(Duration::from_secs(90)))).unwrap();
        assert_eq!(refreshed.token, lock.token);
        assert_eq!(refreshed.timeout, Some(Duration::from_secs(90)));

        let found = ready(ls.discover(&p));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].timeout, Some(Duration::from_secs(90)));
    }
}
