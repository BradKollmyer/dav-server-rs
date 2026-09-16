use std::any::Any;
use std::error::Error as StdError;
use std::io;
use std::pin::pin;
use std::time::SystemTime;

use bytes::{Buf, Bytes};
use headers::HeaderMapExt;
use http::StatusCode as SC;
use http::header::HeaderValue;
use http::{self, Request, Response};
use http_body::Body as HttpBody;
use http_body_util::BodyExt;

use crate::body::Body;
use crate::conditional::if_match_get_tokens;
use crate::davheaders;
use crate::davpath::DavPath;
use crate::fs::*;
use crate::{DavError, DavInner, DavResult};

const SABRE: &str = "application/x-sabredav-partialupdate";

/// Parent collection that requires iCalendar / vCard object validation on PUT.
#[cfg(any(feature = "caldav", feature = "carddav"))]
#[derive(Clone, Copy)]
enum TypedCollection {
    #[cfg(feature = "caldav")]
    Calendar,
    #[cfg(feature = "carddav")]
    Addressbook,
}

#[cfg(any(feature = "caldav", feature = "carddav"))]
impl TypedCollection {
    /// Limit for calendar/addressbook object PUT/PATCH in this collection.
    fn max_resource_size(self) -> u64 {
        match self {
            #[cfg(feature = "caldav")]
            TypedCollection::Calendar => crate::caldav::DEFAULT_MAX_RESOURCE_SIZE,
            #[cfg(feature = "carddav")]
            TypedCollection::Addressbook => crate::carddav::DEFAULT_MAX_RESOURCE_SIZE,
        }
    }
}

/// Size of the resource after a PUT (replace) or PATCH/partial PUT.
#[cfg(any(feature = "caldav", feature = "carddav"))]
fn resulting_resource_size(
    do_range: bool,
    append: bool,
    existing: u64,
    start: u64,
    written: u64,
) -> u64 {
    if do_range {
        if append {
            existing.saturating_add(written)
        } else {
            existing.max(start.saturating_add(written))
        }
    } else {
        written
    }
}

// This is a nice hack. If the type 'E' is actually an io::Error or a Box<io::Error>,
// convert it back into a real io::Error. If it is a DavError or a Box<DavError>,
// use its Into<io::Error> impl. Otherwise just wrap the error in io::Error::new.
//
// If we had specialization this would look a lot prettier.
//
// Also, this is senseless. It's not as if we _do_ anything with the
// io::Error, other than noticing "oops an error occured".
fn to_ioerror<E>(err: E) -> io::Error
where
    E: StdError + Sync + Send + 'static,
{
    let e = &err as &dyn Any;
    if e.is::<io::Error>() || e.is::<Box<io::Error>>() {
        let err = Box::new(err) as Box<dyn Any>;
        match err.downcast::<io::Error>() {
            Ok(e) => *e,
            Err(e) => match e.downcast::<Box<io::Error>>() {
                Ok(e) => *(*e),
                Err(_) => io::ErrorKind::Other.into(),
            },
        }
    } else if e.is::<DavError>() || e.is::<Box<DavError>>() {
        let err = Box::new(err) as Box<dyn Any>;
        match err.downcast::<DavError>() {
            Ok(e) => (*e).into(),
            Err(e) => match e.downcast::<Box<DavError>>() {
                Ok(e) => (*(*e)).into(),
                Err(_) => io::ErrorKind::Other.into(),
            },
        }
    } else {
        io::Error::other(err)
    }
}

impl<C: Clone + Send + Sync + 'static> DavInner<C> {
    pub(crate) async fn handle_put<ReqBody, ReqData, ReqError>(
        self,
        req: &Request<()>,
        body: ReqBody,
    ) -> DavResult<Response<Body>>
    where
        ReqBody: HttpBody<Data = ReqData, Error = ReqError>,
        ReqData: Buf + Send + 'static,
        ReqError: StdError + Send + Sync + 'static,
    {
        let mut start = 0;
        let mut count = 0;
        let mut have_count = false;
        let mut do_range = false;

        let mut oo = OpenOptions::write();
        oo.create = true;
        oo.truncate = true;

        if let Some(n) = req.headers().typed_get::<headers::ContentLength>() {
            count = n.0;
            have_count = true;
            oo.size = Some(count);
        } else if let Some(n) = req
            .headers()
            .get("X-Expected-Entity-Length")
            .and_then(|v| v.to_str().ok())
        {
            // macOS Finder, see https://evertpot.com/260/
            if let Ok(len) = n.parse() {
                count = len;
                have_count = true;
                oo.size = Some(count);
            }
        }
        let checksum = req
            .headers()
            .get("OC-Checksum")
            .and_then(|v| v.to_str().ok().map(|s| s.to_string()));
        oo.checksum = checksum;

        let (oc_mtime, oc_ctime) = Self::oc_timestamps(req, true)?;

        let path = self.path(req);
        self.ensure_visible(&path).await?;
        let meta = self.fs.metadata(&path, &self.credentials).await;
        // RFC 4918 9.7.2: PUT/PATCH on a collection is 405.
        if let Ok(ref m) = meta
            && m.is_dir()
        {
            return Err(DavError::StatusClose(SC::METHOD_NOT_ALLOWED));
        }

        // close connection on error.
        let mut res = Response::new(Body::empty());
        res.headers_mut().typed_insert(headers::Connection::close());

        // SabreDAV style PATCH?
        if req.method() == http::Method::PATCH {
            if req
                .headers()
                .typed_get::<davheaders::ContentType>()
                .is_none_or(|ct| ct.0 != SABRE)
            {
                return Err(DavError::StatusClose(SC::UNSUPPORTED_MEDIA_TYPE));
            }
            if !have_count {
                return Err(DavError::StatusClose(SC::LENGTH_REQUIRED));
            };
            let r = req
                .headers()
                .typed_get::<davheaders::XUpdateRange>()
                .ok_or(DavError::StatusClose(SC::BAD_REQUEST))?;
            match r {
                davheaders::XUpdateRange::FromTo(b, e) => {
                    if b > e || e - b + 1 != count {
                        return Err(DavError::StatusClose(SC::RANGE_NOT_SATISFIABLE));
                    }
                    start = b;
                }
                davheaders::XUpdateRange::AllFrom(b) => {
                    start = b;
                }
                davheaders::XUpdateRange::Last(n) => {
                    if let Ok(ref m) = meta {
                        if n > m.len() {
                            return Err(DavError::StatusClose(SC::RANGE_NOT_SATISFIABLE));
                        }
                        start = m.len() - n;
                    }
                }
                davheaders::XUpdateRange::Append => {
                    oo.append = true;
                }
            }
            do_range = true;
            oo.truncate = false;
        }

        // Apache-style Content-Range header?
        match req.headers().typed_try_get::<headers::ContentRange>() {
            Ok(Some(range)) => {
                if let Some((b, e)) = range.bytes_range() {
                    if b > e {
                        return Err(DavError::StatusClose(SC::RANGE_NOT_SATISFIABLE));
                    }

                    if have_count {
                        if e - b + 1 != count {
                            return Err(DavError::StatusClose(SC::RANGE_NOT_SATISFIABLE));
                        }
                    } else {
                        count = e - b + 1;
                        have_count = true;
                    }
                    start = b;
                    do_range = true;
                    oo.truncate = false;
                }
            }
            Ok(None) => {}
            Err(_) => return Err(DavError::StatusClose(SC::BAD_REQUEST)),
        }

        // The end of the range must fit in a u64, and in a usize for
        // filesystems that keep the file in memory.
        if do_range
            && !oo.append
            && start
                .checked_add(count)
                .is_none_or(|end| usize::try_from(end).is_err())
        {
            return Err(DavError::StatusClose(SC::RANGE_NOT_SATISFIABLE));
        }

        // check the If and If-* headers.
        let tokens = if_match_get_tokens(
            req,
            meta.as_ref().map(|v| v.as_ref()).ok(),
            self.fs.as_ref(),
            &self.ls,
            &path,
            &self.credentials,
        );
        let tokens = match tokens.await {
            Ok(t) => t,
            Err(s) => return Err(DavError::StatusClose(s)),
        };

        // if locked check if we hold that lock.
        if let Some(ref locksystem) = self.ls {
            let principal = self.principal.as_deref();
            if let Err(_l) = locksystem
                .check(&path, principal, false, false, &tokens)
                .await
            {
                return Err(DavError::StatusClose(SC::LOCKED));
            }
        }

        #[cfg(any(feature = "caldav", feature = "carddav"))]
        let typed_collection = self.typed_parent_collection(&path).await;
        #[cfg(any(feature = "caldav", feature = "carddav"))]
        let size_limit = typed_collection.map(TypedCollection::max_resource_size);
        #[cfg(any(feature = "caldav", feature = "carddav"))]
        let existing_len = meta.as_ref().map(|m| m.len()).unwrap_or(0);
        #[cfg(any(feature = "caldav", feature = "carddav"))]
        let restore_bytes = if do_range && typed_collection.is_some() && meta.is_ok() {
            self.read_resource_bytes(&path, existing_len).await.ok()
        } else {
            None
        };
        #[cfg(any(feature = "caldav", feature = "carddav"))]
        if let Some(max) = size_limit
            && have_count
        {
            let resulting =
                resulting_resource_size(do_range, oo.append, existing_len, start, count);
            if resulting > max {
                return Err(DavError::StatusClose(SC::FORBIDDEN));
            }
        }

        // tweak open options.
        if req
            .headers()
            .typed_get::<davheaders::IfMatch>()
            .is_some_and(|h| h.0 == davheaders::ETagList::Star)
        {
            oo.create = false;
        }
        if req
            .headers()
            .typed_get::<davheaders::IfNoneMatch>()
            .is_some_and(|h| h.0 == davheaders::ETagList::Star)
        {
            oo.create_new = true;
        }

        let create = oo.create;
        let create_new = oo.create_new;
        #[cfg(any(feature = "caldav", feature = "carddav"))]
        let append = oo.append;
        let mut file = match self.fs.open(&path, oo, &self.credentials).await {
            Ok(f) => f,
            Err(FsError::NotFound) | Err(FsError::Exists) => {
                let s = if !create || create_new {
                    SC::PRECONDITION_FAILED
                } else {
                    SC::CONFLICT
                };
                return Err(DavError::StatusClose(s));
            }
            Err(e) => return Err(DavError::FsError(e)),
        };

        if do_range {
            // seek to beginning of requested data.
            if file.seek(std::io::SeekFrom::Start(start)).await.is_err() {
                return Err(DavError::StatusClose(SC::RANGE_NOT_SATISFIABLE));
            }
        }

        res.headers_mut()
            .typed_insert(headers::AcceptRanges::bytes());

        let mut body = pin!(body);

        #[cfg(any(feature = "caldav", feature = "carddav"))]
        let mut typed_body = (!do_range && typed_collection.is_some()).then(Vec::new);

        // loop, read body, write to file.
        let written: DavResult<()> = async {
            let mut total = 0u64;
            while let Some(data) = body.frame().await {
                let data_frame = data.map_err(|e| to_ioerror(e))?;

                let Ok(mut buf) = data_frame.into_data() else {
                    continue;
                };

                total += buf.remaining() as u64;
                #[cfg(any(feature = "caldav", feature = "carddav"))]
                if let Some(max) = size_limit {
                    let resulting =
                        resulting_resource_size(do_range, append, existing_len, start, total);
                    if resulting > max {
                        return Err(DavError::StatusClose(SC::FORBIDDEN));
                    }
                }
                // consistency check.
                if have_count && total > count {
                    break;
                }
                // The `Buf` might actually be a `Bytes`.
                let b = {
                    let b: &mut dyn std::any::Any = &mut buf;
                    b.downcast_mut::<Bytes>()
                };
                if let Some(bytes) = b {
                    let bytes = std::mem::replace(bytes, Bytes::new());
                    #[cfg(any(feature = "caldav", feature = "carddav"))]
                    if let Some(ref mut collected) = typed_body {
                        collected.extend_from_slice(&bytes);
                    }
                    file.write_bytes(bytes).await?;
                } else {
                    #[cfg(any(feature = "caldav", feature = "carddav"))]
                    if let Some(ref mut collected) = typed_body {
                        let bytes = buf.copy_to_bytes(buf.remaining());
                        collected.extend_from_slice(&bytes);
                        file.write_bytes(bytes).await?;
                        continue;
                    }
                    file.write_buf(Box::new(buf)).await?;
                }
            }
            file.flush().await?;
            drop(file);

            if have_count && total > count {
                error!("PUT file: sender is sending more bytes than expected");
                return Err(DavError::StatusClose(SC::BAD_REQUEST));
            }

            if have_count && total < count {
                error!("PUT file: premature EOF on input");
                return Err(DavError::StatusClose(SC::BAD_REQUEST));
            }
            Ok(())
        }
        .await;

        // A failed write into a calendar/addressbook collection must not leave
        // a half-written object behind: restore the previous PATCH target, or
        // remove a newly created resource.
        #[cfg(any(feature = "caldav", feature = "carddav"))]
        if let Err(e) = written {
            if typed_collection.is_some() {
                if do_range {
                    self.restore_or_remove_typed(&path, restore_bytes).await;
                } else if meta.is_err() {
                    let _ = self.fs.remove_file(&path, &self.credentials).await;
                }
            }
            return Err(e);
        }
        #[cfg(not(any(feature = "caldav", feature = "carddav")))]
        written?;

        // RFC 4791 5.3.2.1 / RFC 6352 6.3.2: invalid calendar/address data is 403.
        // Full PUT is checked from the collected body; PATCH / partial PUT is
        // checked from the resulting resource and restored on failure.
        #[cfg(any(feature = "caldav", feature = "carddav"))]
        if let Some(kind) = typed_collection {
            let body = if do_range {
                let cap = size_limit.unwrap_or(u64::MAX);
                self.read_resource_bytes(&path, cap).await.ok()
            } else {
                typed_body.take()
            };
            if !body.is_some_and(|body| Self::typed_body_valid(kind, &body)) {
                self.restore_or_remove_typed(&path, restore_bytes).await;
                return Err(DavError::StatusClose(SC::FORBIDDEN));
            }
        }

        // Report whether we created or updated the file.
        *res.status_mut() = match meta {
            Ok(_) => SC::NO_CONTENT,
            Err(_) => {
                res.headers_mut().typed_insert(headers::ContentLength(0));
                SC::CREATED
            }
        };

        // no errors, connection may be kept open.
        res.headers_mut().remove(http::header::CONNECTION);

        self.apply_oc_timestamps(&path, oc_mtime, oc_ctime, &mut res)
            .await;

        if let Ok(meta) = self.fs.metadata(&path, &self.credentials).await {
            if let Some(etag) = davheaders::ETag::from_meta(meta.as_ref()) {
                res.headers_mut().typed_insert(etag);
            }
            if let Ok(modified) = meta.modified() {
                res.headers_mut()
                    .typed_insert(headers::LastModified::from(modified));
            }
        }
        Ok(res)
    }

    /// Parent calendar or addressbook collection, if this PUT needs object validation.
    #[cfg(any(feature = "caldav", feature = "carddav"))]
    async fn typed_parent_collection(&self, path: &DavPath) -> Option<TypedCollection> {
        let parent = path.parent();
        let meta = self.fs.metadata(&parent, &self.credentials).await.ok()?;
        #[cfg(feature = "caldav")]
        if self.collection_is_calendar(&parent, meta.as_ref()).await {
            return Some(TypedCollection::Calendar);
        }
        #[cfg(feature = "carddav")]
        if self.collection_is_addressbook(&parent, meta.as_ref()).await {
            return Some(TypedCollection::Addressbook);
        }
        None
    }

    #[cfg(any(feature = "caldav", feature = "carddav"))]
    fn typed_body_valid(kind: TypedCollection, body: &[u8]) -> bool {
        match std::str::from_utf8(body) {
            Ok(text) => match kind {
                #[cfg(feature = "caldav")]
                TypedCollection::Calendar => crate::caldav::validate_calendar_data(text).is_ok(),
                #[cfg(feature = "carddav")]
                TypedCollection::Addressbook => crate::carddav::validate_vcard_data(text).is_ok(),
            },
            Err(_) => false,
        }
    }

    #[cfg(any(feature = "caldav", feature = "carddav"))]
    async fn read_resource_bytes(&self, path: &DavPath, max: u64) -> FsResult<Vec<u8>> {
        let mut file = self
            .fs
            .open(path, OpenOptions::read(), &self.credentials)
            .await?;
        let n = file.metadata().await?.len().min(max) as usize;
        read_file_to_end(file.as_mut(), n).await
    }

    #[cfg(any(feature = "caldav", feature = "carddav"))]
    async fn restore_resource_bytes(&self, path: &DavPath, bytes: Vec<u8>) -> FsResult<()> {
        let mut oo = OpenOptions::write();
        oo.create = true;
        oo.truncate = true;
        let mut file = self.fs.open(path, oo, &self.credentials).await?;
        file.write_bytes(Bytes::from(bytes)).await?;
        file.flush().await
    }

    #[cfg(any(feature = "caldav", feature = "carddav"))]
    async fn restore_or_remove_typed(&self, path: &DavPath, restore: Option<Vec<u8>>) {
        if let Some(prev) = restore
            && self.restore_resource_bytes(path, prev).await.is_ok()
        {
            return;
        }
        let _ = self.fs.remove_file(path, &self.credentials).await;
    }

    /// Parse ownCloud/Nextcloud `X-OC-MTime` / `X-OC-CTime` headers.
    /// Malformed values yield 400. `close` selects `StatusClose` vs `Status`.
    pub(crate) fn oc_timestamps(
        req: &Request<()>,
        close: bool,
    ) -> DavResult<(Option<SystemTime>, Option<SystemTime>)> {
        let bad_request = || {
            if close {
                DavError::StatusClose(SC::BAD_REQUEST)
            } else {
                DavError::Status(SC::BAD_REQUEST)
            }
        };
        let mtime = match req.headers().typed_try_get::<davheaders::XOcMTime>() {
            Ok(v) => v.map(|h| h.0),
            Err(_) => return Err(bad_request()),
        };
        let ctime = match req.headers().typed_try_get::<davheaders::XOcCTime>() {
            Ok(v) => v.map(|h| h.0),
            Err(_) => return Err(bad_request()),
        };
        Ok((mtime, ctime))
    }

    /// Apply client-supplied timestamps and echo `accepted` when the backend honors them.
    pub(crate) async fn apply_oc_timestamps(
        &self,
        path: &DavPath,
        mtime: Option<SystemTime>,
        ctime: Option<SystemTime>,
        res: &mut Response<Body>,
    ) {
        if let Some(tm) = mtime {
            match self.fs.set_modified(path, tm, &self.credentials).await {
                Ok(()) => {
                    res.headers_mut().insert(
                        davheaders::X_OC_MTIME.clone(),
                        HeaderValue::from_static("accepted"),
                    );
                }
                Err(FsError::NotImplemented) => {}
                Err(e) => debug!("failed to apply X-OC-MTime: {e:?}"),
            }
        }
        if let Some(tm) = ctime {
            match self.fs.set_created(path, tm, &self.credentials).await {
                Ok(()) => {
                    res.headers_mut().insert(
                        davheaders::X_OC_CTIME.clone(),
                        HeaderValue::from_static("accepted"),
                    );
                }
                Err(FsError::NotImplemented) => {}
                Err(e) => debug!("failed to apply X-OC-CTime: {e:?}"),
            }
        }
    }
}
