use std::io::Cursor;

use headers::HeaderMapExt;
use http::{Request, Response, StatusCode};
use xmltree::Element;

use crate::body::Body;
use crate::conditional::*;
use crate::davheaders;
#[cfg(any(feature = "proppatch", feature = "caldav", feature = "carddav"))]
use crate::davpath::DavPath;
use crate::fs::*;
use crate::xmltree_ext::ElementExt;
use crate::{DavError, DavInner, DavResult};

const NS_DAV_URI: &str = "DAV:";

impl<C: Clone + Send + Sync + 'static> DavInner<C> {
    /// Undo a newly created collection, including backend type-marker sidecars.
    /// Only reserved marker files are removed; never recursively delete contents
    /// that another request may have created in the meantime.
    #[cfg(any(feature = "proppatch", feature = "caldav", feature = "carddav"))]
    pub(crate) async fn rollback_created_collection(&self, path: &DavPath) {
        for name in [b".dav-calendar".as_slice(), b".dav-addressbook".as_slice()] {
            let mut marker = path.clone();
            marker.push_segment(name);
            match self.fs.remove_file(&marker, &self.credentials).await {
                Ok(()) | Err(FsError::NotFound | FsError::NotImplemented) => {}
                Err(e) => debug!("failed to remove collection marker during rollback: {e:?}"),
            }
        }
        if let Err(e) = self.fs.remove_dir(path, &self.credentials).await {
            debug!("failed to remove newly created collection during rollback: {e:?}");
        }
    }

    /// RFC 4918 MKCOL, plus extended MKCOL (RFC 5689).
    ///
    /// A `DAV:mkcol` body can set properties and, when CalDAV/CardDAV are
    /// enabled, mark the new collection as a calendar or address book via
    /// `DAV:resourcetype`. Nested calendar-in-calendar (RFC 4791 4.2) and
    /// addressbook-in-addressbook (RFC 6352 5.2) are 403.
    pub(crate) async fn handle_mkcol(
        &self,
        req: &Request<()>,
        body: &[u8],
    ) -> DavResult<Response<Body>> {
        let mut path = self.path(req);
        self.ensure_visible(&path).await?;
        let (oc_mtime, oc_ctime) = Self::oc_timestamps(req, false)?;
        let meta = self.fs.metadata(&path, &self.credentials).await;

        // check the If and If-* headers.
        let res = if_match_get_tokens(
            req,
            meta.as_ref().map(|v| v.as_ref()).ok(),
            self.fs.as_ref(),
            &self.ls,
            &path,
            &self.credentials,
        )
        .await;
        let tokens = match res {
            Ok(t) => t,
            Err(s) => return Err(DavError::Status(s)),
        };

        // if locked check if we hold that lock.
        if let Some(ref locksystem) = self.ls {
            let principal = self.principal.as_deref();
            if let Err(_l) = locksystem
                .check(&path, principal, false, false, &tokens)
                .await
            {
                return Err(DavError::Status(StatusCode::LOCKED));
            }
        }

        let mkcol_body = if body.is_empty() {
            None
        } else {
            // RFC 4918 9.3.1: an unsupported MKCOL entity type is 415.
            // RFC 5689 `DAV:mkcol` is the only body we accept.
            match Element::parse2(Cursor::new(body)) {
                Ok(tree)
                    if tree.name == "mkcol" && tree.namespace.as_deref() == Some(NS_DAV_URI) =>
                {
                    Some(tree)
                }
                _ => {
                    return Err(DavError::StatusClose(StatusCode::UNSUPPORTED_MEDIA_TYPE));
                }
            }
        };

        #[allow(unused_variables)]
        let (want_calendar, want_addressbook) = mkcol_body
            .as_ref()
            .map(resourcetype_flags)
            .unwrap_or((false, false));

        #[cfg(any(feature = "caldav", feature = "carddav"))]
        {
            let parent = path.parent();
            if (want_calendar || want_addressbook)
                && let Ok(meta) = self.fs.metadata(&parent, &self.credentials).await
            {
                let nested = match self.collection_kind(&parent, meta.as_ref()).await {
                    #[cfg(feature = "caldav")]
                    CollectionKind::Calendar => want_calendar,
                    #[cfg(feature = "carddav")]
                    CollectionKind::Addressbook => want_addressbook,
                    CollectionKind::None => false,
                };
                if nested {
                    return Err(DavError::Status(StatusCode::FORBIDDEN));
                }
            }
        }

        let mut res = Response::new(Body::empty());

        match self.fs.create_dir(&path, &self.credentials).await {
            // RFC 4918 9.3.1 MKCOL Status Codes.
            Err(FsError::Exists) => return Err(DavError::Status(StatusCode::METHOD_NOT_ALLOWED)),
            Err(FsError::NotFound) => return Err(DavError::Status(StatusCode::CONFLICT)),
            Err(e) => return Err(DavError::FsError(e)),
            Ok(()) => {
                if path.is_collection() {
                    path.add_slash();
                    res.headers_mut().typed_insert(davheaders::ContentLocation(
                        path.with_prefix().as_url_string(),
                    ));
                }
                *res.status_mut() = StatusCode::CREATED;
            }
        }

        // A failed type marker must not leave a plain collection behind
        // (RFC 4791 5.3.1.2), so undo the create_dir before failing.
        #[cfg(feature = "caldav")]
        if want_calendar && let Err(e) = self.fs.mark_calendar(&path, &self.credentials).await {
            self.rollback_created_collection(&path).await;
            return Err(e.into());
        }
        #[cfg(feature = "carddav")]
        if want_addressbook && let Err(e) = self.fs.mark_addressbook(&path, &self.credentials).await
        {
            self.rollback_created_collection(&path).await;
            return Err(e.into());
        }

        if let Some(tree) = mkcol_body {
            #[cfg(feature = "proppatch")]
            if let Err(e) = self.apply_set_props(&path, &tree).await {
                self.rollback_created_collection(&path).await;
                return Err(e);
            }
            #[cfg(not(feature = "proppatch"))]
            let _ = tree;
        }

        self.apply_oc_timestamps(&path, oc_mtime, oc_ctime, &mut res)
            .await;

        Ok(res)
    }
}

fn resourcetype_flags(tree: &Element) -> (bool, bool) {
    let mut calendar = false;
    let mut addressbook = false;
    for rt in tree
        .child_elems_iter()
        .filter(|e| e.name == "set")
        .flat_map(|e| e.child_elems_iter())
        .filter(|e| e.name == "prop")
        .flat_map(|e| e.child_elems_iter())
        .filter(|e| e.name == "resourcetype")
        .flat_map(|e| e.child_elems_iter())
    {
        calendar |= is_calendar_resourcetype(rt);
        addressbook |= is_addressbook_resourcetype(rt);
    }
    (calendar, addressbook)
}

#[cfg(feature = "caldav")]
fn is_calendar_resourcetype(rt: &Element) -> bool {
    rt.name == "calendar" && rt.namespace.as_deref() == Some(crate::caldav::NS_CALDAV_URI)
}

#[cfg(not(feature = "caldav"))]
fn is_calendar_resourcetype(_rt: &Element) -> bool {
    false
}

#[cfg(feature = "carddav")]
fn is_addressbook_resourcetype(rt: &Element) -> bool {
    rt.name == "addressbook" && rt.namespace.as_deref() == Some(crate::carddav::NS_CARDDAV_URI)
}

#[cfg(not(feature = "carddav"))]
fn is_addressbook_resourcetype(_rt: &Element) -> bool {
    false
}
