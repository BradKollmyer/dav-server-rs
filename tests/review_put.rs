#![cfg(all(feature = "memfs", any(feature = "caldav", feature = "carddav")))]

use bytes::Bytes;
use dav_server::{DavHandler, body::Body, memfs::MemFs};
#[cfg(feature = "caldav")]
use dav_server::{
    davpath::DavPath,
    fs::{
        DavDirEntry, DavFile, DavFileSystem, DavMetaData, FsError, FsFuture, FsStream, OpenOptions,
        ReadDirMeta,
    },
};
use http::{Request, StatusCode};
use http_body_util::BodyExt;
#[cfg(feature = "caldav")]
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

async fn request(
    server: &DavHandler,
    method: &str,
    path: &str,
    body: &str,
    length: Option<usize>,
) -> (StatusCode, String, Option<http::HeaderValue>) {
    let mut req = Request::builder().method(method).uri(path);
    if let Some(length) = length {
        req = req.header("Content-Length", length);
    }
    let response = server.handle(req.body(Body::from(body)).unwrap()).await;
    let status = response.status();
    let etag = response.headers().get("ETag").cloned();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8(bytes.to_vec()).unwrap(), etag)
}

async fn rejected_replacement_preserves_resource(method: &str, data: &str) {
    let server = DavHandler::builder()
        .filesystem(MemFs::new())
        .build_handler();
    assert_eq!(
        request(&server, method, "/collection", "", None).await.0,
        StatusCode::CREATED
    );
    assert_eq!(
        request(&server, "PUT", "/collection/item", data, None)
            .await
            .0,
        StatusCode::CREATED
    );
    let before = request(&server, "GET", "/collection/item", "", None).await;
    for (body, length, expected) in [
        ("invalid".to_owned(), None, StatusCode::FORBIDDEN),
        (
            data.to_owned(),
            Some(data.len() + 1),
            StatusCode::BAD_REQUEST,
        ),
        (data.to_owned(), Some(1), StatusCode::BAD_REQUEST),
        ("x".repeat(1024 * 1024 + 1), None, StatusCode::FORBIDDEN),
    ] {
        assert_eq!(
            request(&server, "PUT", "/collection/item", &body, length)
                .await
                .0,
            expected
        );
        assert_eq!(
            request(&server, "GET", "/collection/item", "", None).await,
            before
        );
    }
    let stream = futures_util::stream::iter([
        Ok(http_body::Frame::data(Bytes::from_static(b"partial"))),
        Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "upload interrupted",
        )),
    ]);
    let response = server
        .handle(
            Request::builder()
                .method("PUT")
                .uri("/collection/item")
                .body(http_body_util::StreamBody::new(stream))
                .unwrap(),
        )
        .await;
    assert!(!response.status().is_success());
    assert_eq!(
        request(&server, "GET", "/collection/item", "", None).await,
        before
    );
    assert_eq!(
        request(&server, "PUT", "/collection/item", data, None)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
}

#[cfg(feature = "caldav")]
const ICS: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Test//EN\r\nBEGIN:VEVENT\r\nUID:one\r\nDTSTART:20260101T120000Z\r\nSUMMARY:Original\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

#[cfg(feature = "caldav")]
#[tokio::test]
async fn rejected_calendar_replacements_preserve_bytes_and_etag() {
    rejected_replacement_preserves_resource("MKCALENDAR", ICS).await;
}

#[cfg(feature = "carddav")]
#[tokio::test]
async fn rejected_contact_replacements_preserve_bytes_and_etag() {
    rejected_replacement_preserves_resource(
        "MKADDRESSBOOK",
        "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:one\r\nFN:Alice\r\nEND:VCARD\r\n",
    )
    .await;
}

#[derive(Clone)]
#[cfg(feature = "caldav")]
struct FailingFs {
    inner: MemFs,
    fail_write: Arc<AtomicBool>,
    fail_read: Arc<AtomicBool>,
}

#[derive(Debug)]
#[cfg(feature = "caldav")]
struct FailingFile {
    inner: Box<dyn DavFile>,
    fail: Arc<AtomicBool>,
}

#[cfg(feature = "caldav")]
impl DavFileSystem for FailingFs {
    fn symlink_metadata<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, Box<dyn DavMetaData>> {
        self.inner.symlink_metadata(path)
    }
    fn metadata<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, Box<dyn DavMetaData>> {
        self.inner.metadata(path)
    }
    fn read_dir<'a>(
        &'a self,
        path: &'a DavPath,
        meta: ReadDirMeta,
    ) -> FsFuture<'a, FsStream<Box<dyn DavDirEntry>>> {
        self.inner.read_dir(path, meta)
    }
    fn open<'a>(
        &'a self,
        path: &'a DavPath,
        options: OpenOptions,
    ) -> FsFuture<'a, Box<dyn DavFile>> {
        Box::pin(async move {
            if options.read && self.fail_read.load(Ordering::SeqCst) {
                return Err(FsError::Forbidden);
            }
            let inner = self.inner.open(path, options).await?;
            Ok(Box::new(FailingFile {
                inner,
                fail: self.fail_write.clone(),
            }) as Box<dyn DavFile>)
        })
    }
    fn create_dir<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, ()> {
        self.inner.create_dir(path)
    }
    fn remove_file<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, ()> {
        self.inner.remove_file(path)
    }
    #[cfg(feature = "caldav")]
    fn mark_calendar<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, ()> {
        self.inner.mark_calendar(path)
    }
}

#[cfg(feature = "caldav")]
impl DavFile for FailingFile {
    fn metadata(&mut self) -> FsFuture<'_, Box<dyn DavMetaData>> {
        self.inner.metadata()
    }
    fn read_bytes(&mut self, count: usize) -> FsFuture<'_, Bytes> {
        self.inner.read_bytes(count)
    }
    fn write_bytes(&mut self, buf: Bytes) -> FsFuture<'_, ()> {
        Box::pin(async move {
            if self.fail.swap(false, Ordering::SeqCst) {
                self.inner
                    .write_bytes(buf.slice(..buf.len().min(10)))
                    .await?;
                return Err(FsError::GeneralFailure);
            }
            self.inner.write_bytes(buf).await
        })
    }
    fn write_buf(&mut self, buf: Box<dyn bytes::Buf + Send>) -> FsFuture<'_, ()> {
        self.inner.write_buf(buf)
    }
    fn seek(&mut self, pos: std::io::SeekFrom) -> FsFuture<'_, u64> {
        self.inner.seek(pos)
    }
    fn flush(&mut self) -> FsFuture<'_, ()> {
        self.inner.flush()
    }
}

#[cfg(feature = "caldav")]
#[tokio::test]
async fn calendar_backend_write_failure_restores_existing_bytes() {
    let fs = FailingFs {
        inner: *MemFs::new(),
        fail_write: Arc::default(),
        fail_read: Arc::default(),
    };
    let server = DavHandler::builder()
        .filesystem(Box::new(fs.clone()))
        .build_handler();
    assert_eq!(
        request(&server, "MKCALENDAR", "/cal", "", None).await.0,
        StatusCode::CREATED
    );
    assert_eq!(
        request(&server, "PUT", "/cal/item", ICS, None).await.0,
        StatusCode::CREATED
    );
    fs.fail_write.store(true, Ordering::SeqCst);
    assert!(
        !request(
            &server,
            "PUT",
            "/cal/item",
            &ICS.replace("Original", "Replacement"),
            None
        )
        .await
        .0
        .is_success()
    );
    assert_eq!(request(&server, "GET", "/cal/item", "", None).await.1, ICS);
    fs.fail_read.store(true, Ordering::SeqCst);
    assert!(
        !request(
            &server,
            "PUT",
            "/cal/item",
            &ICS.replace("Original", "Replacement"),
            None
        )
        .await
        .0
        .is_success()
    );
    fs.fail_read.store(false, Ordering::SeqCst);
    assert_eq!(request(&server, "GET", "/cal/item", "", None).await.1, ICS);
}
