//! Simple in-memory locksystem.
//!
//! This implementation has state - if you create a
//! new instance in a handler(), it will be empty every time.
//!
//! This means you have to create the instance once, using `MemLs::new`, store
//! it in your handler struct, and clone() it every time you pass
//! it to the DavHandler. As a MemLs struct is just a handle, cloning is cheap.
//!
//! Expired locks are dropped before lock, unlock, refresh, check, and
//! discover. `discover` returns locks that cover the path: the resource
//! itself plus `Depth: infinity` ancestors, not `Depth: 0` ancestors.
use std::collections::HashMap;
use std::future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use futures_util::FutureExt;
use uuid::Uuid;
use xmltree::Element;

use crate::davpath::DavPath;
use crate::fs::FsResult;
use crate::ls::*;
use crate::tree;

type Tree = tree::Tree<Vec<u8>, Vec<DavLock>>;

/// Ephemeral in-memory LockSystem.
#[derive(Debug, Clone)]
pub struct MemLs(Arc<Mutex<MemLsInner>>);

#[derive(Debug)]
struct MemLsInner {
    tree: Tree,
    #[allow(dead_code)]
    locks: HashMap<Vec<u8>, u64>,
}

impl MemLs {
    /// Create a new "memls" locksystem.
    pub fn new() -> Box<MemLs> {
        let inner = MemLsInner {
            tree: Tree::new(Vec::new()),
            locks: HashMap::new(),
        };
        Box::new(MemLs(Arc::new(Mutex::new(inner))))
    }

    fn inner(&self) -> std::sync::MutexGuard<'_, MemLsInner> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl DavLockSystem for MemLs {
    fn lock(
        &'_ self,
        path: &DavPath,
        principal: Option<&str>,
        owner: Option<&Element>,
        timeout: Option<Duration>,
        shared: bool,
        deep: bool,
    ) -> LsFuture<'_, Result<DavLock, DavLock>> {
        let inner = &mut *self.inner();
        prune_expired_locks(&mut inner.tree);

        // any locks in the path?
        let rc = check_locks_to_path(&inner.tree, path, None, true, &Vec::new(), shared);
        trace!("lock: check_locks_to_path: {rc:?}");
        if let Err(err) = rc {
            return future::ready(Err(err)).boxed();
        }

        // if it's a deep lock we need to check if there are locks furter along the path.
        if deep {
            let rc = check_locks_from_path(&inner.tree, path, None, true, &Vec::new(), shared);
            trace!("lock: check_locks_from_path: {rc:?}");
            if let Err(err) = rc {
                return future::ready(Err(err)).boxed();
            }
        }

        // create lock.
        let node = get_or_create_path_node(&mut inner.tree, path);
        let timeout_at = timeout.map(|d| SystemTime::now() + d);
        let lock = DavLock {
            token: Uuid::new_v4().urn().to_string(),
            path: Box::new(path.clone()),
            principal: principal.map(|s| s.to_string()),
            owner: owner.map(|o| Box::new(o.clone())),
            timeout_at,
            timeout,
            shared,
            deep,
        };
        trace!("lock {} created", &lock.token);
        let slock = lock.clone();
        node.push(slock);
        future::ready(Ok(lock)).boxed()
    }

    fn unlock(&'_ self, path: &DavPath, token: &str) -> LsFuture<'_, Result<(), ()>> {
        let inner = &mut *self.inner();
        prune_expired_locks(&mut inner.tree);
        let node_id = match lookup_lock(&inner.tree, path, token) {
            None => {
                trace!("unlock: {token} not found at {path}");
                return future::ready(Err(())).boxed();
            }
            Some(n) => n,
        };
        let len = {
            let node = inner.tree.get_node_mut(node_id).unwrap();
            let idx = node.iter().position(|n| n.token.as_str() == token).unwrap();
            node.remove(idx);
            node.len()
        };
        if len == 0 {
            inner.tree.delete_node(node_id).ok();
        }
        future::ready(Ok(())).boxed()
    }

    fn refresh(
        &'_ self,
        path: &DavPath,
        token: &str,
        timeout: Option<Duration>,
    ) -> LsFuture<'_, Result<DavLock, ()>> {
        trace!("refresh lock {token}");
        let inner = &mut *self.inner();
        prune_expired_locks(&mut inner.tree);
        let node_id = match lookup_lock(&inner.tree, path, token) {
            None => {
                trace!("lock not found");
                return future::ready(Err(())).boxed();
            }
            Some(n) => n,
        };
        let node = inner.tree.get_node_mut(node_id).unwrap();
        let idx = node.iter().position(|n| n.token.as_str() == token).unwrap();
        let lock = &mut node[idx];
        let timeout_at = timeout.map(|d| SystemTime::now() + d);
        lock.timeout = timeout;
        lock.timeout_at = timeout_at;
        future::ready(Ok(lock.clone())).boxed()
    }

    fn check(
        &'_ self,
        path: &DavPath,
        principal: Option<&str>,
        ignore_principal: bool,
        deep: bool,
        submitted_tokens: &[String],
    ) -> LsFuture<'_, Result<(), DavLock>> {
        let inner = &mut *self.inner();
        prune_expired_locks(&mut inner.tree);
        let _st = submitted_tokens;
        let rc = check_locks_to_path(
            &inner.tree,
            path,
            principal,
            ignore_principal,
            submitted_tokens,
            false,
        );
        trace!("check: check_lock_to_path: {_st:?}: {rc:?}");
        if let Err(err) = rc {
            return future::ready(Err(err)).boxed();
        }

        // if it's a deep lock we need to check if there are locks furter along the path.
        if deep {
            let rc = check_locks_from_path(
                &inner.tree,
                path,
                principal,
                ignore_principal,
                submitted_tokens,
                false,
            );
            trace!("check: check_locks_from_path: {rc:?}");
            if let Err(err) = rc {
                return future::ready(Err(err)).boxed();
            }
        }
        future::ready(Ok(())).boxed()
    }

    fn discover(&'_ self, path: &DavPath) -> LsFuture<'_, Vec<DavLock>> {
        let inner = &mut *self.inner();
        prune_expired_locks(&mut inner.tree);
        future::ready(list_locks(&inner.tree, path)).boxed()
    }

    fn delete(&'_ self, path: &DavPath) -> LsFuture<'_, Result<(), ()>> {
        let inner = &mut *self.inner();
        if let Some(node_id) = lookup_node(&inner.tree, path) {
            inner.tree.delete_subtree(node_id).ok();
        }
        future::ready(Ok(())).boxed()
    }
}

// Drop locks whose timeout has elapsed so they cannot block new operations.
fn prune_expired_locks(tree: &mut Tree) {
    prune_expired_at(tree, tree::ROOT_ID, SystemTime::now());
}

fn prune_expired_at(tree: &mut Tree, node_id: u64, now: SystemTime) {
    let children: Vec<u64> = match tree.get_children(node_id) {
        Ok(children) => children.map(|(_, id)| id).collect(),
        Err(_) => return,
    };
    for child_id in children {
        prune_expired_at(tree, child_id, now);
    }

    let empty = match tree.get_node_mut(node_id) {
        Ok(node) => {
            node.retain(|lock| !lock.timeout_at.is_some_and(|t| t <= now));
            node.is_empty()
        }
        Err(_) => return,
    };
    if empty {
        // Same as unlock of the last lock: drop empty non-root nodes (root is kept).
        tree.delete_node(node_id).ok();
    }
}

// check if there are any locks along the path.
fn check_locks_to_path(
    tree: &Tree,
    path: &DavPath,
    principal: Option<&str>,
    ignore_principal: bool,
    submitted_tokens: &[String],
    shared_ok: bool,
) -> Result<(), DavLock> {
    // path segments
    let segs = path_to_segs(path, true);
    let last_seg = segs.len() - 1;

    // state
    let mut holds_lock = false;
    let mut first_lock_seen: Option<&DavLock> = None;

    // walk over path segments starting at root.
    let mut node_id = tree::ROOT_ID;
    for (i, seg) in segs.into_iter().enumerate() {
        node_id = match get_child(tree, node_id, seg) {
            Ok(n) => n,
            Err(_) => break,
        };
        let node_locks = match tree.get_node(node_id) {
            Ok(n) => n,
            Err(_) => break,
        };

        for nl in node_locks {
            if i < last_seg && !nl.deep {
                continue;
            }
            if submitted_tokens.iter().any(|t| &nl.token == t)
                && (ignore_principal || principal == nl.principal.as_deref())
            {
                // fine, we hold this lock.
                holds_lock = true;
            } else {
                // exclusive locks are fatal.
                if !nl.shared {
                    return Err(nl.to_owned());
                }
                // remember first shared lock seen.
                if !shared_ok {
                    first_lock_seen.get_or_insert(nl);
                }
            }
        }
    }

    // return conflicting lock on error.
    if !holds_lock && let Some(first_lock_seen) = first_lock_seen {
        return Err(first_lock_seen.to_owned());
    }

    Ok(())
}

// See if there are locks in any path below this collection.
fn check_locks_from_path(
    tree: &Tree,
    path: &DavPath,
    principal: Option<&str>,
    ignore_principal: bool,
    submitted_tokens: &[String],
    shared_ok: bool,
) -> Result<(), DavLock> {
    let node_id = match lookup_node(tree, path) {
        Some(id) => id,
        None => return Ok(()),
    };
    check_locks_from_node(
        tree,
        node_id,
        principal,
        ignore_principal,
        submitted_tokens,
        shared_ok,
    )
}

// See if there are locks in any nodes below this node.
fn check_locks_from_node(
    tree: &Tree,
    node_id: u64,
    principal: Option<&str>,
    ignore_principal: bool,
    submitted_tokens: &[String],
    shared_ok: bool,
) -> Result<(), DavLock> {
    let node_locks = match tree.get_node(node_id) {
        Ok(n) => n,
        Err(_) => return Ok(()),
    };
    for nl in node_locks {
        if (!nl.shared || !shared_ok)
            && (!submitted_tokens.iter().any(|t| t == &nl.token)
                || (!ignore_principal && principal != nl.principal.as_deref()))
        {
            return Err(nl.to_owned());
        }
    }
    if let Ok(children) = tree.get_children(node_id) {
        for (_, node_id) in children {
            check_locks_from_node(
                tree,
                node_id,
                principal,
                ignore_principal,
                submitted_tokens,
                shared_ok,
            )?;
        }
    }
    Ok(())
}

// Find or create node.
fn get_or_create_path_node<'a>(tree: &'a mut Tree, path: &DavPath) -> &'a mut Vec<DavLock> {
    let mut node_id = tree::ROOT_ID;
    for seg in path_to_segs(path, false) {
        node_id = match tree.get_child(node_id, seg) {
            Ok(n) => n,
            Err(_) => tree
                .add_child(node_id, seg.to_vec(), Vec::new(), false)
                .unwrap(),
        };
    }
    tree.get_node_mut(node_id).unwrap()
}

// Find lock in path.
fn lookup_lock(tree: &Tree, path: &DavPath, token: &str) -> Option<u64> {
    trace!("lookup_lock: {token}");

    let mut node_id = tree::ROOT_ID;
    for seg in path_to_segs(path, true) {
        trace!(
            "lookup_lock: node {} seg {}",
            node_id,
            String::from_utf8_lossy(seg)
        );
        node_id = match get_child(tree, node_id, seg) {
            Ok(n) => n,
            Err(_) => break,
        };
        let node = tree.get_node(node_id).unwrap();
        trace!("lookup_lock: locks here: {:?}", &node);
        if node.iter().any(|n| n.token == token) {
            return Some(node_id);
        }
    }
    trace!("lookup_lock: fail");
    None
}

// Find node ID for path.
fn lookup_node(tree: &Tree, path: &DavPath) -> Option<u64> {
    let mut node_id = tree::ROOT_ID;
    for seg in path_to_segs(path, false) {
        node_id = match tree.get_child(node_id, seg) {
            Ok(n) => n,
            Err(_) => return None,
        };
    }
    Some(node_id)
}

// Locks on the request path, plus Depth: infinity locks on ancestors.
fn list_locks(tree: &Tree, path: &DavPath) -> Vec<DavLock> {
    let mut locks = Vec::new();

    let segs = path_to_segs(path, true);
    let last_seg = segs.len() - 1;

    let mut node_id = tree::ROOT_ID;
    for (i, seg) in segs.into_iter().enumerate() {
        node_id = match get_child(tree, node_id, seg) {
            Ok(n) => n,
            Err(_) => break,
        };
        let node = match tree.get_node(node_id) {
            Ok(n) => n,
            Err(_) => break,
        };
        for lock in node {
            // Depth:0 ancestor locks do not cover this resource.
            if i < last_seg && !lock.deep {
                continue;
            }
            locks.push(lock.clone());
        }
    }
    locks
}

fn path_to_segs(path: &DavPath, include_root: bool) -> Vec<&[u8]> {
    let path = path.as_bytes();
    let mut segs: Vec<&[u8]> = path
        .split(|&c| c == b'/')
        .filter(|s| !s.is_empty())
        .collect();
    if include_root {
        segs.insert(0, b"");
    }
    segs
}

fn get_child(tree: &Tree, node_id: u64, seg: &[u8]) -> FsResult<u64> {
    if seg.is_empty() {
        return Ok(node_id);
    }
    tree.get_child(node_id, seg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::davpath::DavPath;
    use futures_util::FutureExt;
    use std::thread;
    use std::time::Duration;

    fn path() -> DavPath {
        DavPath::new("/file").unwrap()
    }

    fn ready<T>(fut: impl std::future::Future<Output = T>) -> T {
        fut.now_or_never()
            .expect("MemLs futures complete immediately")
    }

    #[test]
    fn expired_exclusive_lock_does_not_block_check_or_lock() {
        let ls = MemLs::new();
        let p = path();
        ready(ls.lock(&p, None, None, Some(Duration::from_millis(1)), false, false)).unwrap();
        thread::sleep(Duration::from_millis(50));

        assert!(ready(ls.check(&p, None, true, false, &[])).is_ok());
        assert!(
            ready(ls.lock(&p, None, None, Some(Duration::from_secs(60)), false, false)).is_ok()
        );
    }

    #[test]
    fn discover_omits_expired_locks() {
        let ls = MemLs::new();
        let p = path();
        ready(ls.lock(&p, None, None, Some(Duration::from_millis(1)), false, false)).unwrap();
        thread::sleep(Duration::from_millis(50));

        assert!(ready(ls.discover(&p)).is_empty());
    }

    #[test]
    fn unlock_of_expired_token_is_err() {
        let ls = MemLs::new();
        let p = path();
        let lock =
            ready(ls.lock(&p, None, None, Some(Duration::from_millis(1)), false, false)).unwrap();
        thread::sleep(Duration::from_millis(50));

        assert!(ready(ls.unlock(&p, &lock.token)).is_err());
    }

    #[test]
    fn infinite_timeout_never_expires() {
        let ls = MemLs::new();
        let p = path();
        let lock = ready(ls.lock(&p, None, None, None, false, false)).unwrap();
        thread::sleep(Duration::from_millis(50));

        assert!(ready(ls.check(&p, None, true, false, &[])).is_err());
        let found = ready(ls.discover(&p));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].token, lock.token);
        assert!(ready(ls.lock(&p, None, None, None, false, false)).is_err());
    }

    #[test]
    fn discover_omits_depth_zero_ancestor_lock() {
        let ls = MemLs::new();
        let root = DavPath::new("/").unwrap();
        let child = DavPath::new("/child").unwrap();
        let lock = ready(ls.lock(&root, None, None, None, false, false)).unwrap();

        let found = ready(ls.discover(&child));
        assert!(!found.iter().any(|l| l.token == lock.token));
    }

    #[test]
    fn discover_includes_deep_ancestor_lock() {
        let ls = MemLs::new();
        let root = DavPath::new("/").unwrap();
        let child = DavPath::new("/child").unwrap();
        let lock = ready(ls.lock(&root, None, None, None, false, true)).unwrap();

        let found = ready(ls.discover(&child));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].token, lock.token);
    }

    #[test]
    fn discover_includes_lock_on_request_path() {
        let ls = MemLs::new();
        let child = DavPath::new("/child").unwrap();
        let lock = ready(ls.lock(&child, None, None, None, false, false)).unwrap();

        let found = ready(ls.discover(&child));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].token, lock.token);
    }

    #[test]
    fn recovers_from_poisoned_mutex() {
        let ls = MemLs::new();
        let inner = ls.0.clone();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = inner.lock().unwrap();
            panic!("poison");
        }));
        let p = path();
        assert!(ready(ls.discover(&p)).is_empty());
    }
}
