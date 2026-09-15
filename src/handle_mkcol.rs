use std::io::Cursor;

use headers::HeaderMapExt;
use http::{Request, Response, StatusCode};
use xmltree::Element;

use crate::body::Body;
use crate::conditional::*;
use crate::davheaders;
use crate::fs::*;
use crate::xmltree_ext::ElementExt;
use crate::{DavError, DavInner, DavResult};

const NS_DAV_URI: &str = "DAV:";

impl<C: Clone + Send + Sync + 'static> DavInner<C> {
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
            if let Ok(meta) = self.fs.metadata(&parent, &self.credentials).await {
                #[cfg(feature = "caldav")]
                if want_calendar && self.collection_is_calendar(&parent, meta.as_ref()).await {
                    return Err(DavError::Status(StatusCode::FORBIDDEN));
                }
                #[cfg(feature = "carddav")]
                if want_addressbook && self.collection_is_addressbook(&parent, meta.as_ref()).await
                {
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

        #[cfg(feature = "caldav")]
        if want_calendar {
            self.fs.mark_calendar(&path, &self.credentials).await?;
        }
        #[cfg(feature = "carddav")]
        if want_addressbook {
            self.fs.mark_addressbook(&path, &self.credentials).await?;
        }

        if let Some(tree) = mkcol_body {
            #[cfg(feature = "proppatch")]
            self.apply_set_props(&path, &tree).await?;
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
        #[cfg(feature = "caldav")]
        if rt.name == "calendar" && rt.namespace.as_deref() == Some(crate::caldav::NS_CALDAV_URI) {
            calendar = true;
        }
        #[cfg(feature = "carddav")]
        if rt.name == "addressbook"
            && rt.namespace.as_deref() == Some(crate::carddav::NS_CARDDAV_URI)
        {
            addressbook = true;
        }
        let _ = rt;
    }
    (calendar, addressbook)
}
