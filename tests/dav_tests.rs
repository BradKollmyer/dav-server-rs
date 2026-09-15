#[cfg(all(unix, feature = "localfs"))]
mod dav_tests {
    use dav_server::{DavHandler, DavOptionHide, body::Body, fakels::FakeLs, localfs::LocalFs};
    use http::{Request, StatusCode};

    fn setup_dav_server_symlink() -> DavHandler {
        let _ = std::fs::create_dir("/tmp/DAV_SERVER_TEST");
        let _ = std::fs::create_dir("/tmp/DAV_SERVER_TEST/normal_dir");
        let _ = std::fs::create_dir("/tmp/DAV_SERVER_TEST/.hidden_folder");
        let _ = std::os::unix::fs::symlink(
            "/tmp/DAV_SERVER_TEST/normal_dir",
            "/tmp/DAV_SERVER_TEST/symlink_to_dir",
        );

        DavHandler::builder()
            // We need LocalFs to test for symlinks
            .filesystem(LocalFs::new("/tmp/DAV_SERVER_TEST", true, false, false))
            .locksystem(FakeLs::new())
            .autoindex(true)
            .hide_symlinks(true)
            .hide_dot_prefix(DavOptionHide::Always)
            .build_handler()
    }

    async fn resp_to_string(mut resp: http::Response<Body>) -> String {
        use futures_util::StreamExt;

        let mut data = Vec::new();
        let body = resp.body_mut();

        while let Some(chunk) = body.next().await {
            match chunk {
                Ok(bytes) => data.extend_from_slice(&bytes),
                Err(e) => panic!("Error reading body stream: {}", e),
            }
        }

        String::from_utf8(data).unwrap_or_else(|_| "".to_string())
    }

    #[tokio::test]
    async fn test_dav_symlink_propfind_one() {
        let server = setup_dav_server_symlink();

        let req = Request::builder()
            .method("PROPFIND")
            .uri("/symlink_to_dir")
            .header("Depth", "0")
            .body(Body::empty())
            .unwrap();

        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_dav_symlink_propfind_dir() {
        let server = setup_dav_server_symlink();

        let req = Request::builder()
            .method("PROPFIND")
            .uri("/")
            .header("Depth", "1")
            .body(Body::empty())
            .unwrap();

        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::MULTI_STATUS);
        let resp_text = resp_to_string(resp).await;
        assert!(!resp_text.contains("/symlink_to_dir"));
    }

    #[tokio::test]
    async fn test_dav_symlink_get_autoindex_one() {
        let server = setup_dav_server_symlink();

        let req = Request::builder()
            .method("GET")
            .uri("/symlink_to_dir")
            .body(Body::empty())
            .unwrap();

        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_dav_symlink_get_autoindex_dir() {
        let server = setup_dav_server_symlink();

        let req = Request::builder()
            .method("GET")
            .uri("/")
            .body(Body::empty())
            .unwrap();

        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let resp_text = resp_to_string(resp).await;
        assert!(!resp_text.contains("/symlink_to_dir"));
    }

    #[tokio::test]
    async fn test_dav_dotprefix_propfind_one() {
        let server = setup_dav_server_symlink();

        let req = Request::builder()
            .method("PROPFIND")
            .uri("/.hidden_folder")
            .header("Depth", "0")
            .body(Body::empty())
            .unwrap();

        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_dav_dotprefix_propfind_dir() {
        let server = setup_dav_server_symlink();

        let req = Request::builder()
            .method("PROPFIND")
            .uri("/")
            .header("Depth", "1")
            .body(Body::empty())
            .unwrap();

        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::MULTI_STATUS);
        let resp_text = resp_to_string(resp).await;
        assert!(!resp_text.contains("/.hidden_folder"));
    }

    #[tokio::test]
    async fn test_dav_dotprefix_get_autoindex_one() {
        let server = setup_dav_server_symlink();

        let req = Request::builder()
            .method("GET")
            .uri("/.hidden_folder")
            .body(Body::empty())
            .unwrap();

        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_dav_dotprefix_get_autoindex_dir() {
        let server = setup_dav_server_symlink();

        let req = Request::builder()
            .method("GET")
            .uri("/")
            .body(Body::empty())
            .unwrap();

        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let resp_text = resp_to_string(resp).await;
        assert!(!resp_text.contains("/.hidden_folder"));
    }
}

#[cfg(feature = "memfs")]
mod multipart_range_tests {
    use dav_server::{DavHandler, body::Body, fakels::FakeLs, memfs::MemFs};
    use http::{Request, StatusCode};

    fn setup() -> DavHandler {
        DavHandler::builder()
            .filesystem(MemFs::new())
            .locksystem(FakeLs::new())
            .build_handler()
    }

    async fn resp_to_string(mut resp: http::Response<Body>) -> String {
        use futures_util::StreamExt;

        let mut data = Vec::new();
        let body = resp.body_mut();

        while let Some(chunk) = body.next().await {
            match chunk {
                Ok(bytes) => data.extend_from_slice(&bytes),
                Err(e) => panic!("Error reading body stream: {}", e),
            }
        }

        String::from_utf8(data).unwrap_or_else(|_| "".to_string())
    }

    #[tokio::test]
    async fn multipart_range_uses_crlf_delimiters() {
        let server = setup();

        let resp = server
            .handle(
                Request::builder()
                    .method("PUT")
                    .uri("/range.txt")
                    .body(Body::from("abc"))
                    .unwrap(),
            )
            .await;
        assert_eq!(resp.status(), StatusCode::CREATED);

        let resp = server
            .handle(
                Request::builder()
                    .method("GET")
                    .uri("/range.txt")
                    .header("Range", "bytes=0-0,2-2")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            content_type.contains("multipart/byteranges"),
            "expected multipart/byteranges, got {content_type}"
        );

        let body = resp_to_string(resp).await;
        assert!(
            body.contains("\r\n--BOUNDARY\r\n"),
            "missing CRLF multipart delimiter in {body:?}"
        );
        assert!(
            !body.contains("\n--BOUNDARY\n"),
            "Unix-newline delimiter must not be used: {body:?}"
        );
        assert!(
            body.contains("\r\n--BOUNDARY--\r\n"),
            "missing CRLF close-delimiter in {body:?}"
        );
    }
}

#[cfg(feature = "memfs")]
mod oc_timestamp_tests {
    use dav_server::{DavHandler, body::Body, fakels::FakeLs, memfs::MemFs};
    use http::{Request, StatusCode};

    const MTIME: &str = "1675789581";
    const CTIME: &str = "1577836800";
    const MTIME_HTTP: &str = "Tue, 07 Feb 2023 17:06:21 GMT";
    const CTIME_RFC3339: &str = "2020-01-01T00:00:00Z";

    fn setup() -> DavHandler {
        DavHandler::builder()
            .filesystem(MemFs::new())
            .locksystem(FakeLs::new())
            .build_handler()
    }

    async fn resp_to_string(mut resp: http::Response<Body>) -> String {
        use futures_util::StreamExt;

        let mut data = Vec::new();
        let body = resp.body_mut();

        while let Some(chunk) = body.next().await {
            match chunk {
                Ok(bytes) => data.extend_from_slice(&bytes),
                Err(e) => panic!("Error reading body stream: {}", e),
            }
        }

        String::from_utf8(data).unwrap_or_else(|_| "".to_string())
    }

    async fn propfind_body(server: &DavHandler, uri: &str) -> String {
        let req = Request::builder()
            .method("PROPFIND")
            .uri(uri)
            .header("Depth", "0")
            .body(Body::from(
                r#"<?xml version="1.0" encoding="utf-8"?>
<D:propfind xmlns:D="DAV:">
  <D:prop>
    <D:creationdate/>
    <D:getlastmodified/>
  </D:prop>
</D:propfind>"#,
            ))
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::MULTI_STATUS);
        resp_to_string(resp).await
    }

    #[tokio::test]
    async fn put_oc_mtime_and_ctime() {
        let server = setup();

        let req = Request::builder()
            .method("PUT")
            .uri("/notes.txt")
            .header("X-OC-MTime", MTIME)
            .header("X-OC-CTime", CTIME)
            .body(Body::from("hello"))
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::CREATED);
        assert_eq!(
            resp.headers()
                .get("x-oc-mtime")
                .and_then(|v| v.to_str().ok()),
            Some("accepted")
        );
        assert_eq!(
            resp.headers()
                .get("x-oc-ctime")
                .and_then(|v| v.to_str().ok()),
            Some("accepted")
        );
        assert_eq!(
            resp.headers()
                .get("last-modified")
                .and_then(|v| v.to_str().ok()),
            Some(MTIME_HTTP)
        );

        let body = propfind_body(&server, "/notes.txt").await;
        assert!(
            body.contains(MTIME_HTTP),
            "getlastmodified missing in {body}"
        );
        assert!(
            body.contains(CTIME_RFC3339),
            "creationdate missing in {body}"
        );
    }

    #[tokio::test]
    async fn put_without_oc_headers_has_no_accepted() {
        let server = setup();

        let req = Request::builder()
            .method("PUT")
            .uri("/plain.txt")
            .body(Body::from("hello"))
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::CREATED);
        assert!(resp.headers().get("x-oc-mtime").is_none());
        assert!(resp.headers().get("x-oc-ctime").is_none());
    }

    #[tokio::test]
    async fn put_malformed_oc_mtime_is_bad_request() {
        let server = setup();

        let req = Request::builder()
            .method("PUT")
            .uri("/bad.txt")
            .header("X-OC-MTime", "not-a-timestamp")
            .body(Body::from("hello"))
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let req = Request::builder()
            .method("GET")
            .uri("/bad.txt")
            .body(Body::empty())
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn put_overwrite_updates_mtime() {
        let server = setup();

        let req = Request::builder()
            .method("PUT")
            .uri("/notes.txt")
            .body(Body::from("v1"))
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::CREATED);

        let req = Request::builder()
            .method("PUT")
            .uri("/notes.txt")
            .header("X-OC-MTime", MTIME)
            .body(Body::from("v2"))
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            resp.headers()
                .get("x-oc-mtime")
                .and_then(|v| v.to_str().ok()),
            Some("accepted")
        );
        assert_eq!(
            resp.headers()
                .get("last-modified")
                .and_then(|v| v.to_str().ok()),
            Some(MTIME_HTTP)
        );
    }

    #[tokio::test]
    async fn mkcol_oc_mtime_and_ctime() {
        let server = setup();

        let req = Request::builder()
            .method("MKCOL")
            .uri("/photos")
            .header("X-OC-MTime", MTIME)
            .header("X-OC-CTime", CTIME)
            .body(Body::empty())
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::CREATED);
        assert_eq!(
            resp.headers()
                .get("x-oc-mtime")
                .and_then(|v| v.to_str().ok()),
            Some("accepted")
        );
        assert_eq!(
            resp.headers()
                .get("x-oc-ctime")
                .and_then(|v| v.to_str().ok()),
            Some("accepted")
        );

        let body = propfind_body(&server, "/photos").await;
        assert!(
            body.contains(MTIME_HTTP),
            "getlastmodified missing in {body}"
        );
        assert!(
            body.contains(CTIME_RFC3339),
            "creationdate missing in {body}"
        );
    }

    #[tokio::test]
    async fn mkcol_malformed_oc_mtime_is_bad_request() {
        let server = setup();

        let req = Request::builder()
            .method("MKCOL")
            .uri("/bad-dir")
            .header("X-OC-MTime", "-1")
            .body(Body::empty())
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}

#[cfg(feature = "memfs")]
mod put_collection_tests {
    use dav_server::{DavHandler, body::Body, fakels::FakeLs, memfs::MemFs};
    use http::{Request, StatusCode};

    fn setup() -> DavHandler {
        DavHandler::builder()
            .filesystem(MemFs::new())
            .locksystem(FakeLs::new())
            .build_handler()
    }

    #[tokio::test]
    async fn put_on_collection_is_method_not_allowed() {
        let server = setup();

        let req = Request::builder()
            .method("MKCOL")
            .uri("/dir")
            .body(Body::empty())
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::CREATED);

        let req = Request::builder()
            .method("PUT")
            .uri("/dir")
            .body(Body::from("nope"))
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
    }
}

#[cfg(feature = "localfs")]
mod oc_timestamp_localfs_tests {
    use dav_server::{DavHandler, body::Body, localfs::LocalFs};
    use http::{Request, StatusCode};
    use std::time::{Duration, UNIX_EPOCH};

    const MTIME: u64 = 1675789581;
    const MTIME_HTTP: &str = "Tue, 07 Feb 2023 17:06:21 GMT";

    #[tokio::test]
    async fn put_oc_mtime_localfs() {
        let dir = std::env::temp_dir().join(format!(
            "dav-oc-mtime-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let server = DavHandler::builder()
            .filesystem(LocalFs::new(&dir, true, false, false))
            .build_handler();

        let req = Request::builder()
            .method("PUT")
            .uri("/notes.txt")
            .header("X-OC-MTime", MTIME.to_string())
            .body(Body::from("hello"))
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::CREATED);
        assert_eq!(
            resp.headers()
                .get("x-oc-mtime")
                .and_then(|v| v.to_str().ok()),
            Some("accepted")
        );
        assert_eq!(
            resp.headers()
                .get("last-modified")
                .and_then(|v| v.to_str().ok()),
            Some(MTIME_HTTP)
        );

        let meta = std::fs::metadata(dir.join("notes.txt")).unwrap();
        let modified = meta.modified().unwrap();
        let expected = UNIX_EPOCH + Duration::from_secs(MTIME);
        let delta = modified
            .duration_since(expected)
            .or_else(|_| expected.duration_since(modified))
            .unwrap();
        assert!(
            delta < Duration::from_secs(1),
            "localfs mtime {modified:?} not close to {expected:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(all(unix, feature = "localfs"))]
mod localfs_umask_tests {
    use dav_server::{DavHandler, body::Body, localfs::LocalFs};
    use http::{Request, StatusCode};
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};

    static UMASK_LOCK: Mutex<()> = Mutex::new(());

    struct UmaskGuard(libc::mode_t);

    impl UmaskGuard {
        fn set(mask: libc::mode_t) -> Self {
            let old = unsafe { libc::umask(mask) };
            Self(old)
        }
    }

    impl Drop for UmaskGuard {
        fn drop(&mut self) {
            unsafe {
                libc::umask(self.0);
            }
        }
    }

    fn unix_mode(path: &std::path::Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    fn tempdir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "dav-umask-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // umask is process-global; lock must cover the awaits
    async fn put_and_mkcol_honor_umask() {
        let _lock = UMASK_LOCK.lock().unwrap();
        let _umask = UmaskGuard::set(0o002);
        let dir = tempdir();

        let server = DavHandler::builder()
            .filesystem(LocalFs::new(&dir, true, false, false))
            .build_handler();

        let put = server
            .handle(
                Request::builder()
                    .method("PUT")
                    .uri("/group.txt")
                    .body(Body::from("hi"))
                    .unwrap(),
            )
            .await;
        assert!(put.status().is_success(), "{}", put.status());
        assert_eq!(
            unix_mode(&dir.join("group.txt")),
            0o664,
            "PUT file should be 0664 under umask 002, not 0644"
        );

        let mk = server
            .handle(
                Request::builder()
                    .method("MKCOL")
                    .uri("/album")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(mk.status(), StatusCode::CREATED, "{}", mk.status());
        assert_eq!(
            unix_mode(&dir.join("album")),
            0o775,
            "MKCOL dir should be 0775 under umask 002, not 0755"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(all(feature = "memfs", feature = "proppatch"))]
mod win32_mtime_tests {
    use dav_server::{DavHandler, body::Body, fakels::FakeLs, memfs::MemFs};
    use http::{Request, StatusCode};

    const MTIME_HTTP: &str = "Tue, 07 Feb 2023 17:06:21 GMT";

    fn setup() -> DavHandler {
        DavHandler::builder()
            .filesystem(MemFs::new())
            .locksystem(FakeLs::new())
            .build_handler()
    }

    async fn resp_to_string(mut resp: http::Response<Body>) -> String {
        use futures_util::StreamExt;

        let mut data = Vec::new();
        let body = resp.body_mut();

        while let Some(chunk) = body.next().await {
            match chunk {
                Ok(bytes) => data.extend_from_slice(&bytes),
                Err(e) => panic!("Error reading body stream: {}", e),
            }
        }

        String::from_utf8(data).unwrap_or_else(|_| "".to_string())
    }

    fn proppatch(uri: &str, mtime: &str) -> Request<Body> {
        Request::builder()
            .method("PROPPATCH")
            .uri(uri)
            .body(Body::from(format!(
                r#"<?xml version="1.0" encoding="utf-8"?>
<D:propertyupdate xmlns:D="DAV:" xmlns:Z="urn:schemas-microsoft-com:">
  <D:set>
    <D:prop>
      <Z:Win32LastModifiedTime>{mtime}</Z:Win32LastModifiedTime>
    </D:prop>
  </D:set>
</D:propertyupdate>"#
            )))
            .unwrap()
    }

    #[tokio::test]
    async fn proppatch_win32_last_modified_sets_mtime() {
        let server = setup();

        let resp = server
            .handle(
                Request::builder()
                    .method("PUT")
                    .uri("/notes.txt")
                    .body(Body::from("hello"))
                    .unwrap(),
            )
            .await;
        assert_eq!(resp.status(), StatusCode::CREATED);

        let resp = server.handle(proppatch("/notes.txt", MTIME_HTTP)).await;
        assert_eq!(resp.status(), StatusCode::MULTI_STATUS);
        let body = resp_to_string(resp).await;
        assert!(
            body.contains("200 OK") || body.contains("HTTP/1.1 200"),
            "expected 200 for Win32LastModifiedTime, got {body}"
        );

        let req = Request::builder()
            .method("PROPFIND")
            .uri("/notes.txt")
            .header("Depth", "0")
            .body(Body::from(
                r#"<?xml version="1.0" encoding="utf-8"?>
<D:propfind xmlns:D="DAV:">
  <D:prop>
    <D:getlastmodified/>
  </D:prop>
</D:propfind>"#,
            ))
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::MULTI_STATUS);
        let body = resp_to_string(resp).await;
        assert!(
            body.contains(MTIME_HTTP),
            "getlastmodified missing {MTIME_HTTP} in {body}"
        );
    }

    #[tokio::test]
    async fn proppatch_win32_last_modified_malformed_is_conflict() {
        let server = setup();

        let resp = server
            .handle(
                Request::builder()
                    .method("PUT")
                    .uri("/notes.txt")
                    .body(Body::from("hello"))
                    .unwrap(),
            )
            .await;
        assert_eq!(resp.status(), StatusCode::CREATED);

        let resp = server.handle(proppatch("/notes.txt", "not-a-date")).await;
        assert_eq!(resp.status(), StatusCode::MULTI_STATUS);
        let body = resp_to_string(resp).await;
        assert!(
            body.contains("409") || body.contains("Conflict"),
            "expected 409 for malformed Win32LastModifiedTime, got {body}"
        );
    }
}

#[cfg(all(feature = "localfs", feature = "proppatch"))]
mod win32_mtime_localfs_tests {
    use dav_server::{DavHandler, body::Body, localfs::LocalFs};
    use http::{Request, StatusCode};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    const MTIME: u64 = 1675789581;
    const MTIME_HTTP: &str = "Tue, 07 Feb 2023 17:06:21 GMT";

    #[tokio::test]
    async fn proppatch_win32_last_modified_localfs() {
        let dir = std::env::temp_dir().join(format!(
            "dav-win32-mtime-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let server = DavHandler::builder()
            .filesystem(LocalFs::new(&dir, true, false, false))
            .build_handler();

        let resp = server
            .handle(
                Request::builder()
                    .method("PUT")
                    .uri("/notes.txt")
                    .body(Body::from("hello"))
                    .unwrap(),
            )
            .await;
        assert_eq!(resp.status(), StatusCode::CREATED);

        let resp = server
            .handle(
                Request::builder()
                    .method("PROPPATCH")
                    .uri("/notes.txt")
                    .body(Body::from(format!(
                        r#"<?xml version="1.0" encoding="utf-8"?>
<D:propertyupdate xmlns:D="DAV:" xmlns:Z="urn:schemas-microsoft-com:">
  <D:set>
    <D:prop>
      <Z:Win32LastModifiedTime>{MTIME_HTTP}</Z:Win32LastModifiedTime>
    </D:prop>
  </D:set>
</D:propertyupdate>"#
                    )))
                    .unwrap(),
            )
            .await;
        assert_eq!(resp.status(), StatusCode::MULTI_STATUS);

        let meta = std::fs::metadata(dir.join("notes.txt")).unwrap();
        let modified = meta.modified().unwrap();
        let expected = UNIX_EPOCH + Duration::from_secs(MTIME);
        let delta = modified
            .duration_since(expected)
            .or_else(|_| expected.duration_since(modified))
            .unwrap();
        assert!(
            delta < Duration::from_secs(1),
            "localfs mtime {modified:?} not close to {expected:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(feature = "memfs")]
mod empty_xml_body_tests {
    use dav_server::{DavHandler, body::Body, fakels::FakeLs, memfs::MemFs};
    use http::{Request, StatusCode};

    fn setup() -> DavHandler {
        DavHandler::builder()
            .filesystem(MemFs::new())
            .locksystem(FakeLs::new())
            .build_handler()
    }

    #[tokio::test]
    async fn propfind_whitespace_body_is_allprop() {
        let server = setup();
        let req = Request::builder()
            .method("PROPFIND")
            .uri("/")
            .header("Depth", "0")
            .body(Body::from("\n"))
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::MULTI_STATUS);
    }

    #[tokio::test]
    async fn propfind_xml_declaration_only_is_bad_request() {
        let server = setup();
        let req = Request::builder()
            .method("PROPFIND")
            .uri("/")
            .header("Depth", "0")
            .body(Body::from(r#"<?xml version="1.0"?>"#))
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[cfg(feature = "proppatch")]
    #[tokio::test]
    async fn proppatch_empty_or_rootless_body_is_bad_request() {
        let server = setup();
        let resp = server
            .handle(
                Request::builder()
                    .method("PUT")
                    .uri("/notes.txt")
                    .body(Body::from("hello"))
                    .unwrap(),
            )
            .await;
        assert_eq!(resp.status(), StatusCode::CREATED);

        for body in [
            Body::empty(),
            Body::from("\n"),
            Body::from(r#"<?xml version="1.0"?>"#),
        ] {
            let resp = server
                .handle(
                    Request::builder()
                        .method("PROPPATCH")
                        .uri("/notes.txt")
                        .body(body)
                        .unwrap(),
                )
                .await;
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        }
    }

    #[tokio::test]
    async fn lock_rootless_xml_is_bad_request() {
        let server = setup();
        let resp = server
            .handle(
                Request::builder()
                    .method("PUT")
                    .uri("/notes.txt")
                    .body(Body::from("hello"))
                    .unwrap(),
            )
            .await;
        assert_eq!(resp.status(), StatusCode::CREATED);

        for xml in ["\n", r#"<?xml version="1.0"?>"#] {
            let resp = server
                .handle(
                    Request::builder()
                        .method("LOCK")
                        .uri("/notes.txt")
                        .body(Body::from(xml))
                        .unwrap(),
                )
                .await;
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        }
    }
}

#[cfg(feature = "memfs")]
mod if_state_token_tests {
    use dav_server::{DavHandler, body::Body, memfs::MemFs, memls::MemLs};
    use http::{Request, StatusCode};

    const LOCKINFO: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<D:lockinfo xmlns:D="DAV:">
  <D:lockscope><D:exclusive/></D:lockscope>
  <D:locktype><D:write/></D:locktype>
</D:lockinfo>"#;

    const SHARED_LOCKINFO: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<D:lockinfo xmlns:D="DAV:">
  <D:lockscope><D:shared/></D:lockscope>
  <D:locktype><D:write/></D:locktype>
</D:lockinfo>"#;

    fn setup() -> DavHandler {
        DavHandler::builder()
            .filesystem(MemFs::new())
            .locksystem(MemLs::new())
            .build_handler()
    }

    async fn put(
        server: &DavHandler,
        uri: &str,
        body: &str,
        if_header: Option<&str>,
    ) -> StatusCode {
        let mut builder = Request::builder().method("PUT").uri(uri);
        if let Some(h) = if_header {
            builder = builder.header("If", h);
        }
        let resp = server.handle(builder.body(Body::from(body)).unwrap()).await;
        resp.status()
    }

    async fn lock(server: &DavHandler, uri: &str, depth: &str) -> (StatusCode, Option<String>) {
        lock_xml(server, uri, depth, LOCKINFO).await
    }

    async fn lock_xml(
        server: &DavHandler,
        uri: &str,
        depth: &str,
        xml: &str,
    ) -> (StatusCode, Option<String>) {
        let resp = server
            .handle(
                Request::builder()
                    .method("LOCK")
                    .uri(uri)
                    .header("Depth", depth)
                    .header("Content-Type", "application/xml")
                    .body(Body::from(xml))
                    .unwrap(),
            )
            .await;
        let token = resp
            .headers()
            .get("lock-token")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        (resp.status(), token)
    }

    #[tokio::test]
    async fn unlocked_garbage_token_is_precondition_failed() {
        let server = setup();
        assert_eq!(
            put(&server, "/file.txt", "v1", None).await,
            StatusCode::CREATED
        );

        assert_eq!(
            put(
                &server,
                "/file.txt",
                "v2",
                Some("(<opaquelocktoken:garbage>)")
            )
            .await,
            StatusCode::PRECONDITION_FAILED
        );

        let resp = server
            .handle(
                Request::builder()
                    .method("LOCK")
                    .uri("/file.txt")
                    .header("If", "(<opaquelocktoken:garbage>)")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(resp.status(), StatusCode::PRECONDITION_FAILED);
    }

    #[tokio::test]
    async fn unlocked_not_garbage_token_is_allowed() {
        let server = setup();
        assert_eq!(
            put(&server, "/file.txt", "v1", None).await,
            StatusCode::CREATED
        );
        assert_eq!(
            put(
                &server,
                "/file.txt",
                "v2",
                Some("(Not <opaquelocktoken:garbage>)")
            )
            .await,
            StatusCode::NO_CONTENT
        );
    }

    async fn lock_refresh(server: &DavHandler, uri: &str, if_header: &str) -> StatusCode {
        let resp = server
            .handle(
                Request::builder()
                    .method("LOCK")
                    .uri(uri)
                    .header("If", if_header)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        resp.status()
    }

    #[tokio::test]
    async fn lock_refresh_with_real_token_succeeds() {
        let server = setup();
        assert_eq!(
            put(&server, "/file.txt", "v1", None).await,
            StatusCode::CREATED
        );
        let (status, token) = lock(&server, "/file.txt", "0").await;
        assert_eq!(status, StatusCode::OK);
        let token = token.expect("Lock-Token header");

        assert_eq!(
            lock_refresh(&server, "/file.txt", &format!("({token})")).await,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn lock_refresh_with_garbage_token_is_precondition_failed() {
        let server = setup();
        assert_eq!(
            put(&server, "/file.txt", "v1", None).await,
            StatusCode::CREATED
        );
        let (status, _) = lock(&server, "/file.txt", "0").await;
        assert_eq!(status, StatusCode::OK);

        assert_eq!(
            lock_refresh(&server, "/file.txt", "(<opaquelocktoken:garbage>)").await,
            StatusCode::PRECONDITION_FAILED
        );
    }

    #[tokio::test]
    async fn lock_refresh_not_dav_no_lock_and_real_token_succeeds() {
        let server = setup();
        assert_eq!(
            put(&server, "/file.txt", "v1", None).await,
            StatusCode::CREATED
        );
        let (status, token) = lock(&server, "/file.txt", "0").await;
        assert_eq!(status, StatusCode::OK);
        let token = token.expect("Lock-Token header");

        assert_eq!(
            lock_refresh(
                &server,
                "/file.txt",
                &format!("(Not <DAV:no-lock> {token})")
            )
            .await,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn live_lock_token_allows_put() {
        let server = setup();
        assert_eq!(
            put(&server, "/file.txt", "v1", None).await,
            StatusCode::CREATED
        );

        let (status, token) = lock(&server, "/file.txt", "0").await;
        assert_eq!(status, StatusCode::OK);
        let token = token.expect("Lock-Token header");

        assert_eq!(
            put(&server, "/file.txt", "v2", Some(&format!("({token})"))).await,
            StatusCode::NO_CONTENT
        );
    }

    #[tokio::test]
    async fn depth_zero_lock_does_not_cover_child() {
        let server = setup();
        let (status, token) = lock(&server, "/", "0").await;
        assert_eq!(status, StatusCode::OK);
        let token = token.expect("Lock-Token header");

        assert_eq!(
            put(&server, "/child.txt", "hello", Some(&format!("({token})"))).await,
            StatusCode::PRECONDITION_FAILED
        );
        assert_eq!(
            put(&server, "/child.txt", "hello", None).await,
            StatusCode::CREATED
        );
    }

    async fn unlock(server: &DavHandler, uri: &str, lock_token: &str) -> StatusCode {
        server
            .handle(
                Request::builder()
                    .method("UNLOCK")
                    .uri(uri)
                    .header("Lock-Token", lock_token)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .status()
    }

    #[tokio::test]
    async fn unlock_releases_lock_so_put_without_token_succeeds() {
        let server = setup();
        assert_eq!(
            put(&server, "/file.txt", "v1", None).await,
            StatusCode::CREATED
        );
        let (status, token) = lock(&server, "/file.txt", "0").await;
        assert_eq!(status, StatusCode::OK);
        let token = token.expect("Lock-Token header");

        assert_eq!(
            put(&server, "/file.txt", "blocked", None).await,
            StatusCode::LOCKED
        );
        assert_eq!(
            unlock(&server, "/file.txt", &token).await,
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            put(&server, "/file.txt", "v2", None).await,
            StatusCode::NO_CONTENT
        );
    }

    #[tokio::test]
    async fn unlock_without_token_is_bad_request_wrong_token_is_conflict() {
        let server = setup();
        assert_eq!(
            put(&server, "/file.txt", "v1", None).await,
            StatusCode::CREATED
        );
        let (status, token) = lock(&server, "/file.txt", "0").await;
        assert_eq!(status, StatusCode::OK);
        let token = token.expect("Lock-Token header");

        let missing = server
            .handle(
                Request::builder()
                    .method("UNLOCK")
                    .uri("/file.txt")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(missing.status(), StatusCode::BAD_REQUEST);

        assert_eq!(
            unlock(&server, "/file.txt", "<opaquelocktoken:garbage>").await,
            StatusCode::CONFLICT
        );
        assert_eq!(
            unlock(&server, "/file.txt", &token).await,
            StatusCode::NO_CONTENT
        );
    }

    #[tokio::test]
    async fn locked_resource_rejects_put_delete_move_without_token() {
        let server = setup();
        assert_eq!(
            put(&server, "/file.txt", "v1", None).await,
            StatusCode::CREATED
        );
        let (status, token) = lock(&server, "/file.txt", "0").await;
        assert_eq!(status, StatusCode::OK);
        let token = token.expect("Lock-Token header");

        assert_eq!(
            put(&server, "/file.txt", "v2", None).await,
            StatusCode::LOCKED
        );

        let delete = server
            .handle(
                Request::builder()
                    .method("DELETE")
                    .uri("/file.txt")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(delete.status(), StatusCode::LOCKED);

        let mv = server
            .handle(
                Request::builder()
                    .method("MOVE")
                    .uri("/file.txt")
                    .header("Destination", "/moved.txt")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(mv.status(), StatusCode::LOCKED);

        assert_eq!(
            put(&server, "/file.txt", "v2", Some(&format!("({token})"))).await,
            StatusCode::NO_CONTENT
        );
        let delete_ok = server
            .handle(
                Request::builder()
                    .method("DELETE")
                    .uri("/file.txt")
                    .header("If", format!("({token})"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(delete_ok.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn copy_to_locked_destination_is_locked() {
        let server = setup();
        assert_eq!(
            put(&server, "/src.txt", "src", None).await,
            StatusCode::CREATED
        );
        assert_eq!(
            put(&server, "/dst.txt", "dst", None).await,
            StatusCode::CREATED
        );
        let (status, token) = lock(&server, "/dst.txt", "0").await;
        assert_eq!(status, StatusCode::OK);
        let token = token.expect("Lock-Token header");

        let copy = server
            .handle(
                Request::builder()
                    .method("COPY")
                    .uri("/src.txt")
                    .header("Destination", "/dst.txt")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(copy.status(), StatusCode::LOCKED);

        let copy_ok = server
            .handle(
                Request::builder()
                    .method("COPY")
                    .uri("http://example.com/src.txt")
                    .header("Destination", "/dst.txt")
                    .header("If", format!("<http://example.com/dst.txt> ({token})"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(copy_ok.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn shared_locks_stack_and_block_exclusive() {
        let server = setup();
        assert_eq!(
            put(&server, "/file.txt", "v1", None).await,
            StatusCode::CREATED
        );

        let (status, token1) = lock_xml(&server, "/file.txt", "0", SHARED_LOCKINFO).await;
        assert_eq!(status, StatusCode::OK);
        assert!(token1.is_some());

        let (status, token2) = lock_xml(&server, "/file.txt", "0", SHARED_LOCKINFO).await;
        assert_eq!(status, StatusCode::OK);
        assert!(token2.is_some());
        assert_ne!(token1, token2);

        let (status, _) = lock(&server, "/file.txt", "0").await;
        assert_eq!(status, StatusCode::LOCKED);

        assert_eq!(
            put(&server, "/file.txt", "v2", None).await,
            StatusCode::LOCKED
        );
        assert_eq!(
            put(
                &server,
                "/file.txt",
                "v2",
                Some(&format!("({})", token1.unwrap()))
            )
            .await,
            StatusCode::NO_CONTENT
        );
    }
}

#[cfg(feature = "memfs")]
mod fakels_tests {
    use dav_server::{DavHandler, body::Body, fakels::FakeLs, memfs::MemFs};
    use http::{Request, StatusCode};

    const LOCKINFO: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<D:lockinfo xmlns:D="DAV:">
  <D:lockscope><D:exclusive/></D:lockscope>
  <D:locktype><D:write/></D:locktype>
</D:lockinfo>"#;

    fn setup() -> DavHandler {
        DavHandler::builder()
            .filesystem(MemFs::new())
            .locksystem(FakeLs::new())
            .build_handler()
    }

    async fn put(server: &DavHandler, uri: &str, body: &str) -> StatusCode {
        server
            .handle(
                Request::builder()
                    .method("PUT")
                    .uri(uri)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .status()
    }

    async fn lock(server: &DavHandler, uri: &str) -> (StatusCode, Option<String>) {
        let resp = server
            .handle(
                Request::builder()
                    .method("LOCK")
                    .uri(uri)
                    .header("Depth", "0")
                    .header("Content-Type", "application/xml")
                    .body(Body::from(LOCKINFO))
                    .unwrap(),
            )
            .await;
        let token = resp
            .headers()
            .get("lock-token")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        (resp.status(), token)
    }

    async fn unlock(server: &DavHandler, uri: &str, lock_token: &str) -> StatusCode {
        server
            .handle(
                Request::builder()
                    .method("UNLOCK")
                    .uri(uri)
                    .header("Lock-Token", lock_token)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .status()
    }

    #[tokio::test]
    async fn lock_and_unlock_always_succeed() {
        let server = setup();
        assert_eq!(put(&server, "/file.txt", "v1").await, StatusCode::CREATED);

        let (status, token) = lock(&server, "/file.txt").await;
        assert_eq!(status, StatusCode::OK);
        let token = token.expect("Lock-Token header");
        assert!(token.contains("opaquetoken:"), "{token}");

        assert_eq!(
            unlock(&server, "/file.txt", &token).await,
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            unlock(&server, "/file.txt", "<opaquelocktoken:garbage>").await,
            StatusCode::NO_CONTENT
        );
    }

    #[tokio::test]
    async fn lock_does_not_block_put_or_second_exclusive_lock() {
        let server = setup();
        assert_eq!(put(&server, "/file.txt", "v1").await, StatusCode::CREATED);

        let (status, _) = lock(&server, "/file.txt").await;
        assert_eq!(status, StatusCode::OK);

        assert_eq!(
            put(&server, "/file.txt", "v2").await,
            StatusCode::NO_CONTENT
        );

        let (status2, token2) = lock(&server, "/file.txt").await;
        assert_eq!(status2, StatusCode::OK);
        assert!(token2.is_some());
    }

    async fn resp_to_string(mut resp: http::Response<Body>) -> String {
        use futures_util::StreamExt;

        let mut data = Vec::new();
        let body = resp.body_mut();

        while let Some(chunk) = body.next().await {
            match chunk {
                Ok(bytes) => data.extend_from_slice(&bytes),
                Err(e) => panic!("Error reading body stream: {e}"),
            }
        }

        String::from_utf8(data).unwrap_or_else(|_| "".to_string())
    }

    #[tokio::test]
    async fn lock_then_propfind_lockdiscovery_includes_token() {
        let server = setup();
        assert_eq!(put(&server, "/file.txt", "v1").await, StatusCode::CREATED);

        let (status, token) = lock(&server, "/file.txt").await;
        assert_eq!(status, StatusCode::OK);
        let token = token.expect("Lock-Token header");
        let token_href = token.trim_matches(|c| c == '<' || c == '>');

        let resp = server
            .handle(
                Request::builder()
                    .method("PROPFIND")
                    .uri("/file.txt")
                    .header("Depth", "0")
                    .header("Content-Type", "application/xml")
                    .body(Body::from(
                        r#"<?xml version="1.0" encoding="utf-8"?>
<D:propfind xmlns:D="DAV:">
  <D:prop>
    <D:lockdiscovery/>
  </D:prop>
</D:propfind>"#,
                    ))
                    .unwrap(),
            )
            .await;
        assert_eq!(resp.status(), StatusCode::MULTI_STATUS);
        let body = resp_to_string(resp).await;
        assert!(
            body.contains(token_href),
            "lockdiscovery should include issued token {token_href}: {body}"
        );
    }
}

#[cfg(all(unix, feature = "localfs"))]
mod localfs_symlink_jail_tests {
    use dav_server::davpath::DavPath;
    use dav_server::fs::{DavFileSystem, FsError, OpenOptions};
    use dav_server::{DavHandler, body::Body, localfs::LocalFs};
    use http::{Request, StatusCode};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tempdir(prefix: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "dav-{prefix}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn read_opts() -> OpenOptions {
        OpenOptions {
            read: true,
            ..OpenOptions::default()
        }
    }

    fn write_opts() -> OpenOptions {
        OpenOptions {
            write: true,
            create: true,
            truncate: true,
            ..OpenOptions::default()
        }
    }

    #[tokio::test]
    async fn localfs_open_copy_do_not_follow_symlink_out_of_share() {
        let dir = tempdir("jail");
        let outside = tempdir("outside");
        let secret = outside.join("secret");
        std::fs::write(&secret, "leaked").unwrap();
        std::os::unix::fs::symlink(&secret, dir.join("link")).unwrap();
        std::fs::write(dir.join("src.txt"), "payload").unwrap();

        let fs = LocalFs::new(&dir, true, false, false);
        let link = DavPath::new("/link").unwrap();
        let src = DavPath::new("/src.txt").unwrap();

        let meta = fs.metadata(&link).await;
        assert!(
            matches!(meta, Err(FsError::Forbidden) | Err(FsError::NotFound)),
            "{meta:?}"
        );

        let open_read = fs.open(&link, read_opts()).await;
        assert!(
            matches!(open_read, Err(FsError::Forbidden) | Err(FsError::NotFound)),
            "{open_read:?}"
        );

        let open_write = fs.open(&link, write_opts()).await;
        assert!(open_write.is_err(), "{open_write:?}");
        assert_eq!(std::fs::read_to_string(&secret).unwrap(), "leaked");

        let copy = fs.copy(&src, &link).await;
        assert!(copy.is_err(), "{copy:?}");
        assert_eq!(std::fs::read_to_string(&secret).unwrap(), "leaked");

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[tokio::test]
    async fn localfs_follows_in_share_symlink_on_read_not_write() {
        let dir = tempdir("jail-in");
        std::fs::write(dir.join("inside.txt"), "ok").unwrap();
        std::os::unix::fs::symlink("inside.txt", dir.join("rel_link")).unwrap();

        let fs = LocalFs::new(&dir, true, false, false);
        let link = DavPath::new("/rel_link").unwrap();

        let mut file = fs
            .open(&link, read_opts())
            .await
            .expect("read in-share symlink");
        let bytes = file.read_bytes(16).await.unwrap();
        assert_eq!(&bytes[..], b"ok");

        let put = fs.open(&link, write_opts()).await;
        assert!(put.is_err(), "{put:?}");
        assert_eq!(
            std::fs::read_to_string(dir.join("inside.txt")).unwrap(),
            "ok"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn handler_get_put_copy_through_outside_symlink() {
        let dir = tempdir("jail-http");
        let outside = tempdir("outside-http");
        let secret = outside.join("secret");
        std::fs::write(&secret, "leaked").unwrap();
        std::os::unix::fs::symlink(&secret, dir.join("link")).unwrap();
        std::fs::write(dir.join("src.txt"), "payload").unwrap();

        let server = DavHandler::builder()
            .filesystem(LocalFs::new(&dir, true, false, false))
            // Follow the link so this tests LocalFs confinement, not hide_symlinks.
            .hide_symlinks(false)
            .build_handler();

        let get = server
            .handle(
                Request::builder()
                    .method("GET")
                    .uri("/link")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert!(
            get.status() == StatusCode::FORBIDDEN || get.status() == StatusCode::NOT_FOUND,
            "{}",
            get.status()
        );

        let put = server
            .handle(
                Request::builder()
                    .method("PUT")
                    .uri("/link")
                    .body(Body::from("overwrite"))
                    .unwrap(),
            )
            .await;
        assert!(
            !put.status().is_success(),
            "PUT through outside symlink: {}",
            put.status()
        );
        assert_eq!(std::fs::read_to_string(&secret).unwrap(), "leaked");

        let copy = server
            .handle(
                Request::builder()
                    .method("COPY")
                    .uri("/src.txt")
                    .header("Destination", "/link")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        // Overwrite may delete the symlink first, then copy into the share.
        // Either way the outside target must not be written.
        assert_eq!(std::fs::read_to_string(&secret).unwrap(), "leaked");
        let _ = copy;

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&outside);
    }
}

#[cfg(feature = "memfs")]
mod hide_dot_mutating_tests {
    use dav_server::{DavHandler, DavOptionHide, body::Body, memfs::MemFs};
    use http::{Request, StatusCode};

    fn setup() -> DavHandler {
        DavHandler::builder()
            .filesystem(MemFs::new())
            .hide_dot_prefix(DavOptionHide::Always)
            .build_handler()
    }

    #[tokio::test]
    async fn hide_dot_always_rejects_get_and_put() {
        let server = setup();

        let get = server
            .handle(
                Request::builder()
                    .method("GET")
                    .uri("/.secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(get.status(), StatusCode::NOT_FOUND);

        let put = server
            .handle(
                Request::builder()
                    .method("PUT")
                    .uri("/.secret")
                    .body(Body::from("hidden"))
                    .unwrap(),
            )
            .await;
        assert_eq!(put.status(), StatusCode::NOT_FOUND);

        let visible = server
            .handle(
                Request::builder()
                    .method("PUT")
                    .uri("/visible.txt")
                    .body(Body::from("ok"))
                    .unwrap(),
            )
            .await;
        assert_eq!(visible.status(), StatusCode::CREATED);
    }
}

#[cfg(all(unix, feature = "localfs"))]
mod hide_symlinks_mutating_tests {
    use dav_server::{DavHandler, body::Body, localfs::LocalFs};
    use http::{Request, StatusCode};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn setup() -> (DavHandler, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "dav-hide-symlink-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("target.txt"), "hello").unwrap();
        std::os::unix::fs::symlink("target.txt", dir.join("link.txt")).unwrap();

        let server = DavHandler::builder()
            .filesystem(LocalFs::new(&dir, true, false, false))
            .hide_symlinks(true)
            .build_handler();
        (server, dir)
    }

    #[tokio::test]
    async fn hide_symlinks_rejects_put_delete_copy_through_symlink() {
        let (server, dir) = setup();

        let put = server
            .handle(
                Request::builder()
                    .method("PUT")
                    .uri("/link.txt")
                    .body(Body::from("pwned"))
                    .unwrap(),
            )
            .await;
        assert_eq!(put.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            std::fs::read_to_string(dir.join("target.txt")).unwrap(),
            "hello"
        );

        let delete = server
            .handle(
                Request::builder()
                    .method("DELETE")
                    .uri("/link.txt")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(delete.status(), StatusCode::NOT_FOUND);
        assert!(dir.join("link.txt").exists());
        assert_eq!(
            std::fs::read_to_string(dir.join("target.txt")).unwrap(),
            "hello"
        );

        let copy = server
            .handle(
                Request::builder()
                    .method("COPY")
                    .uri("/link.txt")
                    .header("Destination", "/copied.txt")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(copy.status(), StatusCode::NOT_FOUND);
        assert!(!dir.join("copied.txt").exists());
        assert_eq!(
            std::fs::read_to_string(dir.join("target.txt")).unwrap(),
            "hello"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
#[cfg(feature = "memfs")]
mod propfind_depth_tests {
    use dav_server::{DavHandler, body::Body, fakels::FakeLs, memfs::MemFs};
    use http::{Request, StatusCode};

    fn setup() -> DavHandler {
        DavHandler::builder()
            .filesystem(MemFs::new())
            .locksystem(FakeLs::new())
            .build_handler()
    }

    async fn resp_to_string(mut resp: http::Response<Body>) -> String {
        use futures_util::StreamExt;

        let mut data = Vec::new();
        let body = resp.body_mut();

        while let Some(chunk) = body.next().await {
            match chunk {
                Ok(bytes) => data.extend_from_slice(&bytes),
                Err(e) => panic!("Error reading body stream: {}", e),
            }
        }

        String::from_utf8(data).unwrap_or_else(|_| "".to_string())
    }

    #[tokio::test]
    async fn propfind_missing_depth_is_forbidden_when_infinity_disabled() {
        let server = setup();
        let req = Request::builder()
            .method("PROPFIND")
            .uri("/")
            .body(Body::empty())
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let body = resp_to_string(resp).await;
        assert!(
            body.contains("propfind-finite-depth"),
            "expected propfind-finite-depth in {body}"
        );
    }

    #[tokio::test]
    async fn propfind_explicit_infinity_is_forbidden_when_infinity_disabled() {
        let server = setup();
        let req = Request::builder()
            .method("PROPFIND")
            .uri("/")
            .header("Depth", "infinity")
            .body(Body::empty())
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert_ne!(resp.status(), StatusCode::NOT_IMPLEMENTED);
        let body = resp_to_string(resp).await;
        assert!(
            body.contains("propfind-finite-depth"),
            "expected propfind-finite-depth in {body}"
        );
    }

    #[tokio::test]
    async fn propfind_missing_depth_includes_collection_when_infinity_allowed() {
        let server = DavHandler::builder()
            .filesystem(MemFs::new())
            .locksystem(FakeLs::new())
            .allow_infinity_depth(true)
            .build_handler();

        let resp = server
            .handle(
                Request::builder()
                    .method("PUT")
                    .uri("/child.txt")
                    .body(Body::from("hi"))
                    .unwrap(),
            )
            .await;
        assert_eq!(resp.status(), StatusCode::CREATED);

        let req = Request::builder()
            .method("PROPFIND")
            .uri("/")
            .body(Body::empty())
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::MULTI_STATUS);
        let body = resp_to_string(resp).await;
        assert!(
            body.contains("<D:href>/</D:href>"),
            "expected request-URI collection href in {body}"
        );
        assert!(
            body.contains("/child.txt"),
            "expected child in infinite listing: {body}"
        );
    }

    #[tokio::test]
    async fn propfind_depth_zero_includes_collection() {
        let server = setup();
        let req = Request::builder()
            .method("PROPFIND")
            .uri("/")
            .header("Depth", "0")
            .body(Body::empty())
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::MULTI_STATUS);
        let body = resp_to_string(resp).await;
        assert!(
            body.contains("<D:href>/</D:href>"),
            "expected collection href in {body}"
        );
    }
}

#[cfg(all(feature = "memfs", feature = "proppatch"))]
mod propfind_propstat_tests {
    use dav_server::{DavHandler, body::Body, fakels::FakeLs, memfs::MemFs};
    use http::{Request, StatusCode};

    fn setup() -> DavHandler {
        DavHandler::builder()
            .filesystem(MemFs::new())
            .locksystem(FakeLs::new())
            .build_handler()
    }

    async fn resp_to_string(mut resp: http::Response<Body>) -> String {
        use futures_util::StreamExt;

        let mut data = Vec::new();
        let body = resp.body_mut();

        while let Some(chunk) = body.next().await {
            match chunk {
                Ok(bytes) => data.extend_from_slice(&bytes),
                Err(e) => panic!("Error reading body stream: {}", e),
            }
        }

        String::from_utf8(data).unwrap_or_else(|_| "".to_string())
    }

    async fn put_notes(server: &DavHandler) {
        let resp = server
            .handle(
                Request::builder()
                    .method("PUT")
                    .uri("/notes.txt")
                    .body(Body::from("hello"))
                    .unwrap(),
            )
            .await;
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn propfind_specific_prop_reports_404_for_missing_live_prop() {
        let server = setup();
        put_notes(&server).await;

        let req = Request::builder()
            .method("PROPFIND")
            .uri("/notes.txt")
            .header("Depth", "0")
            .body(Body::from(
                r#"<?xml version="1.0" encoding="utf-8"?>
<D:propfind xmlns:D="DAV:">
  <D:prop>
    <D:displayname/>
    <D:no-such-prop/>
  </D:prop>
</D:propfind>"#,
            ))
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::MULTI_STATUS);
        let body = resp_to_string(resp).await;
        assert!(
            body.contains("no-such-prop"),
            "missing property name absent from {body}"
        );
        assert!(
            body.contains("404"),
            "expected 404 propstat for no-such-prop in {body}"
        );
    }

    #[tokio::test]
    async fn propfind_specific_prop_does_not_leak_unsolicited_dead_props() {
        let server = setup();
        put_notes(&server).await;

        let resp = server
            .handle(
                Request::builder()
                    .method("PROPPATCH")
                    .uri("/notes.txt")
                    .body(Body::from(
                        r#"<?xml version="1.0" encoding="utf-8"?>
<D:propertyupdate xmlns:D="DAV:" xmlns:X="http://example.com/ns">
  <D:set>
    <D:prop>
      <X:dead-color>blue</X:dead-color>
    </D:prop>
  </D:set>
</D:propertyupdate>"#,
                    ))
                    .unwrap(),
            )
            .await;
        assert_eq!(resp.status(), StatusCode::MULTI_STATUS);

        let req = Request::builder()
            .method("PROPFIND")
            .uri("/notes.txt")
            .header("Depth", "0")
            .body(Body::from(
                r#"<?xml version="1.0" encoding="utf-8"?>
<D:propfind xmlns:D="DAV:">
  <D:prop>
    <D:getcontentlength/>
  </D:prop>
</D:propfind>"#,
            ))
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::MULTI_STATUS);
        let body = resp_to_string(resp).await;
        assert!(
            body.contains("getcontentlength"),
            "expected getcontentlength in {body}"
        );
        assert!(
            !body.contains("dead-color"),
            "specific prop leaked unsolicited dead prop: {body}"
        );
    }

    #[tokio::test]
    async fn propfind_allprop_includes_dead_props() {
        let server = setup();
        put_notes(&server).await;

        let resp = server
            .handle(
                Request::builder()
                    .method("PROPPATCH")
                    .uri("/notes.txt")
                    .body(Body::from(
                        r#"<?xml version="1.0" encoding="utf-8"?>
<D:propertyupdate xmlns:D="DAV:" xmlns:X="http://example.com/ns">
  <D:set>
    <D:prop>
      <X:dead-color>blue</X:dead-color>
    </D:prop>
  </D:set>
</D:propertyupdate>"#,
                    ))
                    .unwrap(),
            )
            .await;
        assert_eq!(resp.status(), StatusCode::MULTI_STATUS);

        let req = Request::builder()
            .method("PROPFIND")
            .uri("/notes.txt")
            .header("Depth", "0")
            .body(Body::from(
                r#"<?xml version="1.0" encoding="utf-8"?>
<D:propfind xmlns:D="DAV:">
  <D:allprop/>
</D:propfind>"#,
            ))
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::MULTI_STATUS);
        let body = resp_to_string(resp).await;
        assert!(
            body.contains("dead-color"),
            "allprop missing dead prop in {body}"
        );
    }

    #[tokio::test]
    async fn proppatch_remove_displayname() {
        let server = setup();
        put_notes(&server).await;

        let resp = server
            .handle(
                Request::builder()
                    .method("PROPPATCH")
                    .uri("/notes.txt")
                    .body(Body::from(
                        r#"<?xml version="1.0" encoding="utf-8"?>
<D:propertyupdate xmlns:D="DAV:">
  <D:set>
    <D:prop>
      <D:displayname>My Notes</D:displayname>
    </D:prop>
  </D:set>
</D:propertyupdate>"#,
                    ))
                    .unwrap(),
            )
            .await;
        assert_eq!(resp.status(), StatusCode::MULTI_STATUS);

        let req = Request::builder()
            .method("PROPFIND")
            .uri("/notes.txt")
            .header("Depth", "0")
            .body(Body::from(
                r#"<?xml version="1.0" encoding="utf-8"?>
<D:propfind xmlns:D="DAV:">
  <D:prop>
    <D:displayname/>
  </D:prop>
</D:propfind>"#,
            ))
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::MULTI_STATUS);
        let body = resp_to_string(resp).await;
        assert!(
            body.contains("My Notes"),
            "displayname missing after set: {body}"
        );
        assert!(
            body.contains("200 OK"),
            "displayname set should be 200: {body}"
        );

        let resp = server
            .handle(
                Request::builder()
                    .method("PROPPATCH")
                    .uri("/notes.txt")
                    .body(Body::from(
                        r#"<?xml version="1.0" encoding="utf-8"?>
<D:propertyupdate xmlns:D="DAV:">
  <D:remove>
    <D:prop>
      <D:displayname/>
    </D:prop>
  </D:remove>
</D:propertyupdate>"#,
                    ))
                    .unwrap(),
            )
            .await;
        assert_eq!(resp.status(), StatusCode::MULTI_STATUS);

        let req = Request::builder()
            .method("PROPFIND")
            .uri("/notes.txt")
            .header("Depth", "0")
            .body(Body::from(
                r#"<?xml version="1.0" encoding="utf-8"?>
<D:propfind xmlns:D="DAV:">
  <D:prop>
    <D:displayname/>
  </D:prop>
</D:propfind>"#,
            ))
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::MULTI_STATUS);
        let body = resp_to_string(resp).await;
        assert!(
            !body.contains("My Notes"),
            "displayname still present after remove: {body}"
        );
        assert!(
            body.contains("404"),
            "removed displayname should be 404: {body}"
        );
    }
}

#[cfg(feature = "memfs")]
mod get_delete_copymove_tests {
    use dav_server::{DavHandler, body::Body, fakels::FakeLs, memfs::MemFs};
    use http::{Request, StatusCode, header};

    fn setup() -> DavHandler {
        DavHandler::builder()
            .filesystem(MemFs::new())
            .locksystem(FakeLs::new())
            .build_handler()
    }

    async fn resp_to_bytes(mut resp: http::Response<Body>) -> Vec<u8> {
        use futures_util::StreamExt;

        let mut data = Vec::new();
        let body = resp.body_mut();
        while let Some(chunk) = body.next().await {
            match chunk {
                Ok(bytes) => data.extend_from_slice(&bytes),
                Err(e) => panic!("Error reading body stream: {e}"),
            }
        }
        data
    }

    async fn resp_to_string(resp: http::Response<Body>) -> String {
        String::from_utf8(resp_to_bytes(resp).await).unwrap_or_default()
    }

    async fn put(server: &DavHandler, uri: &str, body: &str) -> StatusCode {
        server
            .handle(
                Request::builder()
                    .method("PUT")
                    .uri(uri)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .status()
    }

    async fn mkcol(server: &DavHandler, uri: &str) -> StatusCode {
        server
            .handle(
                Request::builder()
                    .method("MKCOL")
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .status()
    }

    async fn get(server: &DavHandler, uri: &str) -> http::Response<Body> {
        server
            .handle(
                Request::builder()
                    .method("GET")
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
    }

    fn header_str(resp: &http::Response<Body>, name: &str) -> Option<String> {
        resp.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
    }

    #[tokio::test]
    async fn get_and_head_file() {
        let server = setup();
        assert_eq!(
            put(&server, "/notes.txt", "hello world").await,
            StatusCode::CREATED
        );

        let resp = get(&server, "/notes.txt").await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(header_str(&resp, "accept-ranges").as_deref(), Some("bytes"));
        assert_eq!(header_str(&resp, "content-length").as_deref(), Some("11"));
        assert!(
            header_str(&resp, "content-type")
                .as_deref()
                .is_some_and(|t| t.starts_with("text/plain")),
            "content-type: {:?}",
            header_str(&resp, "content-type")
        );
        assert!(header_str(&resp, "etag").is_some());
        assert_eq!(resp_to_string(resp).await, "hello world");

        let resp = server
            .handle(
                Request::builder()
                    .method("HEAD")
                    .uri("/notes.txt")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(header_str(&resp, "content-length").as_deref(), Some("11"));
        assert_eq!(header_str(&resp, "accept-ranges").as_deref(), Some("bytes"));
        assert!(resp_to_bytes(resp).await.is_empty());
    }

    #[tokio::test]
    async fn get_missing_is_not_found() {
        let server = setup();
        assert_eq!(
            get(&server, "/nope.txt").await.status(),
            StatusCode::NOT_FOUND
        );
        let head = server
            .handle(
                Request::builder()
                    .method("HEAD")
                    .uri("/nope.txt")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(head.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn get_single_range() {
        let server = setup();
        assert_eq!(
            put(&server, "/notes.txt", "hello world").await,
            StatusCode::CREATED
        );

        let resp = server
            .handle(
                Request::builder()
                    .method("GET")
                    .uri("/notes.txt")
                    .header(header::RANGE, "bytes=0-4")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            header_str(&resp, "content-range").as_deref(),
            Some("bytes 0-4/11")
        );
        assert_eq!(header_str(&resp, "content-length").as_deref(), Some("5"));
        assert_eq!(resp_to_string(resp).await, "hello");
    }

    #[tokio::test]
    async fn get_open_and_suffix_ranges() {
        let server = setup();
        assert_eq!(
            put(&server, "/notes.txt", "hello world").await,
            StatusCode::CREATED
        );

        let from = server
            .handle(
                Request::builder()
                    .method("GET")
                    .uri("/notes.txt")
                    .header(header::RANGE, "bytes=6-")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(from.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            header_str(&from, "content-range").as_deref(),
            Some("bytes 6-10/11")
        );
        assert_eq!(resp_to_string(from).await, "world");

        let suffix = server
            .handle(
                Request::builder()
                    .method("GET")
                    .uri("/notes.txt")
                    .header(header::RANGE, "bytes=-5")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(suffix.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            header_str(&suffix, "content-range").as_deref(),
            Some("bytes 6-10/11")
        );
        assert_eq!(resp_to_string(suffix).await, "world");
    }

    #[tokio::test]
    async fn get_unsatisfiable_range() {
        let server = setup();
        assert_eq!(
            put(&server, "/notes.txt", "hello world").await,
            StatusCode::CREATED
        );

        let resp = server
            .handle(
                Request::builder()
                    .method("GET")
                    .uri("/notes.txt")
                    .header(header::RANGE, "bytes=100-200")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(resp.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(
            header_str(&resp, "content-range").as_deref(),
            Some("bytes */11")
        );
        assert!(resp_to_bytes(resp).await.is_empty());
    }

    #[tokio::test]
    async fn get_range_with_huge_last_byte_pos_is_clamped() {
        let server = setup();
        assert_eq!(
            put(&server, "/notes.txt", "hello world").await,
            StatusCode::CREATED
        );

        // RFC 7233 section 2.1: a last-byte-pos beyond the end of the
        // representation means the last byte. This must not overflow.
        let tail = server
            .handle(
                Request::builder()
                    .method("GET")
                    .uri("/notes.txt")
                    .header(header::RANGE, "bytes=5-18446744073709551615")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(tail.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            header_str(&tail, "content-range").as_deref(),
            Some("bytes 5-10/11")
        );
        assert_eq!(header_str(&tail, "content-length").as_deref(), Some("6"));
        let body = resp_to_bytes(tail).await;
        assert_eq!(body.len(), 6);
        assert_eq!(&body[..], b" world");

        let whole = server
            .handle(
                Request::builder()
                    .method("GET")
                    .uri("/notes.txt")
                    .header(header::RANGE, "bytes=0-18446744073709551615")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(whole.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            header_str(&whole, "content-range").as_deref(),
            Some("bytes 0-10/11")
        );
        assert_eq!(header_str(&whole, "content-length").as_deref(), Some("11"));
        assert_eq!(resp_to_string(whole).await, "hello world");
    }

    #[tokio::test]
    async fn get_multipart_ranges() {
        let server = setup();
        assert_eq!(
            put(&server, "/notes.txt", "hello world").await,
            StatusCode::CREATED
        );

        let resp = server
            .handle(
                Request::builder()
                    .method("GET")
                    .uri("/notes.txt")
                    .header(header::RANGE, "bytes=0-0,10-10")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
        let ctype = header_str(&resp, "content-type").unwrap_or_default();
        assert!(
            ctype.contains("multipart/byteranges") && ctype.contains("BOUNDARY"),
            "content-type: {ctype}"
        );
        let body = resp_to_string(resp).await;
        assert!(body.contains("--BOUNDARY"), "{body}");
        assert!(body.contains("bytes 0-0/11"), "{body}");
        assert!(body.contains("bytes 10-10/11"), "{body}");
        assert!(
            body.contains("\nh\n") || body.contains("\r\nh\r\n") || body.contains("h"),
            "{body}"
        );
        assert!(body.contains('d'), "{body}");
        assert!(body.contains("--BOUNDARY--"), "{body}");
    }

    #[tokio::test]
    async fn head_with_range_has_no_body() {
        let server = setup();
        assert_eq!(
            put(&server, "/notes.txt", "hello world").await,
            StatusCode::CREATED
        );

        let resp = server
            .handle(
                Request::builder()
                    .method("HEAD")
                    .uri("/notes.txt")
                    .header(header::RANGE, "bytes=0-4")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            header_str(&resp, "content-range").as_deref(),
            Some("bytes 0-4/11")
        );
        assert_eq!(header_str(&resp, "content-length").as_deref(), Some("5"));
        assert!(resp_to_bytes(resp).await.is_empty());
    }

    #[tokio::test]
    async fn get_if_range_and_if_none_match() {
        let server = setup();
        assert_eq!(
            put(&server, "/notes.txt", "hello world").await,
            StatusCode::CREATED
        );

        let first = get(&server, "/notes.txt").await;
        let etag = header_str(&first, "etag").expect("etag");
        assert_eq!(resp_to_string(first).await, "hello world");

        let matched = server
            .handle(
                Request::builder()
                    .method("GET")
                    .uri("/notes.txt")
                    .header(header::IF_RANGE, &etag)
                    .header(header::RANGE, "bytes=0-4")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(matched.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(resp_to_string(matched).await, "hello");

        let mismatched = server
            .handle(
                Request::builder()
                    .method("GET")
                    .uri("/notes.txt")
                    .header(header::IF_RANGE, "\"not-the-etag\"")
                    .header(header::RANGE, "bytes=0-4")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(mismatched.status(), StatusCode::OK);
        assert_eq!(resp_to_string(mismatched).await, "hello world");

        let not_modified = server
            .handle(
                Request::builder()
                    .method("GET")
                    .uri("/notes.txt")
                    .header(header::IF_NONE_MATCH, &etag)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(not_modified.status(), StatusCode::NOT_MODIFIED);
        assert!(resp_to_bytes(not_modified).await.is_empty());
    }

    #[tokio::test]
    async fn get_directory_redirect_and_autoindex() {
        let server = DavHandler::builder()
            .filesystem(MemFs::new())
            .locksystem(FakeLs::new())
            .autoindex(true)
            .build_handler();

        assert_eq!(mkcol(&server, "/coll").await, StatusCode::CREATED);
        assert_eq!(
            put(&server, "/coll/child.txt", "hi").await,
            StatusCode::CREATED
        );

        let redirect = get(&server, "/coll").await;
        assert_eq!(redirect.status(), StatusCode::FOUND);
        assert_eq!(header_str(&redirect, "location").as_deref(), Some("/coll/"));

        let index = get(&server, "/coll/").await;
        assert_eq!(index.status(), StatusCode::OK);
        assert!(
            header_str(&index, "content-type")
                .as_deref()
                .is_some_and(|t| t.starts_with("text/html")),
            "content-type: {:?}",
            header_str(&index, "content-type")
        );
        let html = resp_to_string(index).await;
        assert!(html.contains("Index of"), "{html}");
        assert!(html.contains("child.txt"), "{html}");
        assert!(html.contains("Parent Directory"), "{html}");

        let head = server
            .handle(
                Request::builder()
                    .method("HEAD")
                    .uri("/coll/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(head.status(), StatusCode::OK);
        assert!(resp_to_bytes(head).await.is_empty());
    }

    #[tokio::test]
    async fn get_directory_without_autoindex_is_method_not_allowed() {
        let server = setup();
        assert_eq!(mkcol(&server, "/coll").await, StatusCode::CREATED);
        assert_eq!(
            get(&server, "/coll/").await.status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
    }

    #[tokio::test]
    async fn get_indexfile_on_collection() {
        let server = DavHandler::builder()
            .filesystem(MemFs::new())
            .locksystem(FakeLs::new())
            .indexfile("index.txt")
            .build_handler();

        assert_eq!(mkcol(&server, "/coll").await, StatusCode::CREATED);
        assert_eq!(
            put(&server, "/coll/index.txt", "welcome").await,
            StatusCode::CREATED
        );

        let resp = get(&server, "/coll/").await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp_to_string(resp).await, "welcome");
    }

    #[tokio::test]
    async fn get_remote_php_webdav_probe() {
        let server = DavHandler::builder()
            .filesystem(MemFs::new())
            .locksystem(FakeLs::new())
            .remote_php_webdav_probe(true)
            .build_handler();
        let resp = get(&server, "/remote.php/webdav/").await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(header_str(&resp, "content-length").as_deref(), Some("0"));
        assert_eq!(header_str(&resp, "accept-ranges").as_deref(), Some("bytes"));
        assert!(resp_to_bytes(resp).await.is_empty());
    }

    #[tokio::test]
    async fn get_remote_php_webdav_probe_default_is_404() {
        let server = setup();
        assert_eq!(
            get(&server, "/remote.php/webdav/").await.status(),
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn delete_file_and_missing() {
        let server = setup();
        assert_eq!(put(&server, "/notes.txt", "bye").await, StatusCode::CREATED);

        let deleted = server
            .handle(
                Request::builder()
                    .method("DELETE")
                    .uri("/notes.txt")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            get(&server, "/notes.txt").await.status(),
            StatusCode::NOT_FOUND
        );

        let missing = server
            .handle(
                Request::builder()
                    .method("DELETE")
                    .uri("/notes.txt")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn delete_collection_recursive() {
        let server = setup();
        assert_eq!(mkcol(&server, "/coll").await, StatusCode::CREATED);
        assert_eq!(mkcol(&server, "/coll/sub").await, StatusCode::CREATED);
        assert_eq!(put(&server, "/coll/a.txt", "a").await, StatusCode::CREATED);
        assert_eq!(
            put(&server, "/coll/sub/b.txt", "b").await,
            StatusCode::CREATED
        );

        let deleted = server
            .handle(
                Request::builder()
                    .method("DELETE")
                    .uri("/coll")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            get(&server, "/coll/a.txt").await.status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            get(&server, "/coll/sub/b.txt").await.status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(get(&server, "/coll/").await.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn delete_collection_depth_zero() {
        let server = setup();
        assert_eq!(mkcol(&server, "/empty").await, StatusCode::CREATED);
        let empty = server
            .handle(
                Request::builder()
                    .method("DELETE")
                    .uri("/empty")
                    .header("Depth", "0")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(empty.status(), StatusCode::NO_CONTENT);

        assert_eq!(mkcol(&server, "/coll").await, StatusCode::CREATED);
        assert_eq!(put(&server, "/coll/a.txt", "a").await, StatusCode::CREATED);
        let nonempty = server
            .handle(
                Request::builder()
                    .method("DELETE")
                    .uri("/coll")
                    .header("Depth", "0")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(nonempty.status(), StatusCode::FORBIDDEN);
        assert_eq!(get(&server, "/coll/a.txt").await.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn delete_depth_one_is_bad_request() {
        let server = setup();
        assert_eq!(mkcol(&server, "/coll").await, StatusCode::CREATED);
        let resp = server
            .handle(
                Request::builder()
                    .method("DELETE")
                    .uri("/coll")
                    .header("Depth", "1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    async fn copy(
        server: &DavHandler,
        src: &str,
        dest: &str,
        overwrite: Option<&str>,
        depth: Option<&str>,
    ) -> StatusCode {
        let mut builder = Request::builder()
            .method("COPY")
            .uri(src)
            .header("Destination", dest);
        if let Some(o) = overwrite {
            builder = builder.header("Overwrite", o);
        }
        if let Some(d) = depth {
            builder = builder.header("Depth", d);
        }
        server
            .handle(builder.body(Body::empty()).unwrap())
            .await
            .status()
    }

    async fn move_(
        server: &DavHandler,
        src: &str,
        dest: &str,
        overwrite: Option<&str>,
        depth: Option<&str>,
    ) -> StatusCode {
        let mut builder = Request::builder()
            .method("MOVE")
            .uri(src)
            .header("Destination", dest);
        if let Some(o) = overwrite {
            builder = builder.header("Overwrite", o);
        }
        if let Some(d) = depth {
            builder = builder.header("Depth", d);
        }
        server
            .handle(builder.body(Body::empty()).unwrap())
            .await
            .status()
    }

    #[tokio::test]
    async fn copy_file_create_and_overwrite() {
        let server = setup();
        assert_eq!(put(&server, "/a.txt", "alpha").await, StatusCode::CREATED);
        assert_eq!(
            copy(&server, "/a.txt", "/b.txt", None, None).await,
            StatusCode::CREATED
        );
        assert_eq!(resp_to_string(get(&server, "/a.txt").await).await, "alpha");
        assert_eq!(resp_to_string(get(&server, "/b.txt").await).await, "alpha");

        assert_eq!(put(&server, "/c.txt", "gamma").await, StatusCode::CREATED);
        assert_eq!(
            copy(&server, "/a.txt", "/c.txt", Some("F"), None).await,
            StatusCode::PRECONDITION_FAILED
        );
        assert_eq!(resp_to_string(get(&server, "/c.txt").await).await, "gamma");

        assert_eq!(
            copy(&server, "/a.txt", "/c.txt", Some("T"), None).await,
            StatusCode::NO_CONTENT
        );
        assert_eq!(resp_to_string(get(&server, "/c.txt").await).await, "alpha");
    }

    #[tokio::test]
    async fn copy_missing_destination_parent_and_same_path() {
        let server = setup();
        assert_eq!(put(&server, "/a.txt", "alpha").await, StatusCode::CREATED);

        let no_dest = server
            .handle(
                Request::builder()
                    .method("COPY")
                    .uri("/a.txt")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(no_dest.status(), StatusCode::BAD_REQUEST);

        assert_eq!(
            copy(&server, "/a.txt", "/missing/b.txt", None, None).await,
            StatusCode::CONFLICT
        );
        assert_eq!(
            copy(&server, "/a.txt", "/a.txt", None, None).await,
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn copy_destination_absolute_url() {
        let server = setup();
        assert_eq!(put(&server, "/a.txt", "alpha").await, StatusCode::CREATED);
        assert_eq!(
            copy(
                &server,
                "http://example.com/a.txt",
                "http://example.com/copied.txt",
                None,
                None
            )
            .await,
            StatusCode::CREATED
        );
        assert_eq!(
            resp_to_string(get(&server, "/copied.txt").await).await,
            "alpha"
        );
        assert_eq!(
            copy(
                &server,
                "http://example.com/a.txt",
                "http://example.com:80/copied-port.txt",
                None,
                None
            )
            .await,
            StatusCode::CREATED
        );
        assert_eq!(
            resp_to_string(get(&server, "/copied-port.txt").await).await,
            "alpha"
        );
    }

    #[tokio::test]
    async fn copy_destination_absolute_url_with_host_header() {
        let server = setup();
        assert_eq!(put(&server, "/a.txt", "alpha").await, StatusCode::CREATED);
        let resp = server
            .handle(
                Request::builder()
                    .method("COPY")
                    .uri("/a.txt")
                    .header("Host", "127.0.0.1:4918")
                    .header("Destination", "http://127.0.0.1:4918/copied.txt")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(resp.status(), StatusCode::CREATED);
        assert_eq!(
            resp_to_string(get(&server, "/copied.txt").await).await,
            "alpha"
        );
    }

    #[tokio::test]
    async fn copy_destination_other_host_is_bad_gateway() {
        let server = setup();
        assert_eq!(put(&server, "/a.txt", "alpha").await, StatusCode::CREATED);
        assert_eq!(
            copy(&server, "/a.txt", "http://evil.example/file", None, None).await,
            StatusCode::BAD_GATEWAY
        );
        assert_eq!(get(&server, "/file").await.status(), StatusCode::NOT_FOUND);
        assert_eq!(resp_to_string(get(&server, "/a.txt").await).await, "alpha");
    }

    #[tokio::test]
    async fn copy_collection_depth_infinity_and_zero() {
        let server = setup();
        assert_eq!(mkcol(&server, "/src").await, StatusCode::CREATED);
        assert_eq!(mkcol(&server, "/src/sub").await, StatusCode::CREATED);
        assert_eq!(put(&server, "/src/a.txt", "a").await, StatusCode::CREATED);
        assert_eq!(
            put(&server, "/src/sub/b.txt", "b").await,
            StatusCode::CREATED
        );

        assert_eq!(
            copy(&server, "/src", "/dst", None, None).await,
            StatusCode::CREATED
        );
        assert_eq!(resp_to_string(get(&server, "/src/a.txt").await).await, "a");
        assert_eq!(resp_to_string(get(&server, "/dst/a.txt").await).await, "a");
        assert_eq!(
            resp_to_string(get(&server, "/dst/sub/b.txt").await).await,
            "b"
        );

        assert_eq!(
            copy(&server, "/src", "/shallow", None, Some("0")).await,
            StatusCode::CREATED
        );
        assert_eq!(
            get(&server, "/shallow/").await.status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
        assert_eq!(
            get(&server, "/shallow/a.txt").await.status(),
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn copy_depth_one_and_move_depth_zero_are_bad_request() {
        let server = setup();
        assert_eq!(mkcol(&server, "/src").await, StatusCode::CREATED);
        assert_eq!(
            copy(&server, "/src", "/dst", None, Some("1")).await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            move_(&server, "/src", "/dst", None, Some("0")).await,
            StatusCode::BAD_REQUEST
        );
    }

    #[tokio::test]
    async fn move_file_and_collection() {
        let server = setup();
        assert_eq!(put(&server, "/a.txt", "alpha").await, StatusCode::CREATED);
        assert_eq!(
            move_(&server, "/a.txt", "/b.txt", None, None).await,
            StatusCode::CREATED
        );
        assert_eq!(get(&server, "/a.txt").await.status(), StatusCode::NOT_FOUND);
        assert_eq!(resp_to_string(get(&server, "/b.txt").await).await, "alpha");

        assert_eq!(put(&server, "/c.txt", "gamma").await, StatusCode::CREATED);
        assert_eq!(
            move_(&server, "/b.txt", "/c.txt", Some("F"), None).await,
            StatusCode::PRECONDITION_FAILED
        );
        assert_eq!(
            move_(&server, "/b.txt", "/c.txt", Some("T"), None).await,
            StatusCode::NO_CONTENT
        );
        assert_eq!(get(&server, "/b.txt").await.status(), StatusCode::NOT_FOUND);
        assert_eq!(resp_to_string(get(&server, "/c.txt").await).await, "alpha");

        assert_eq!(mkcol(&server, "/from").await, StatusCode::CREATED);
        assert_eq!(mkcol(&server, "/from/sub").await, StatusCode::CREATED);
        assert_eq!(put(&server, "/from/a.txt", "a").await, StatusCode::CREATED);
        assert_eq!(
            put(&server, "/from/sub/b.txt", "b").await,
            StatusCode::CREATED
        );
        assert_eq!(
            move_(&server, "/from", "/to", None, None).await,
            StatusCode::CREATED
        );
        assert_eq!(
            get(&server, "/from/a.txt").await.status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(resp_to_string(get(&server, "/to/a.txt").await).await, "a");
        assert_eq!(
            resp_to_string(get(&server, "/to/sub/b.txt").await).await,
            "b"
        );
    }
}

#[cfg(feature = "memfs")]
mod memfs_path_tests {
    use dav_server::{DavHandler, body::Body, fakels::FakeLs, memfs::MemFs};
    use futures_util::StreamExt;
    use http::{Request, StatusCode};

    fn setup() -> DavHandler {
        DavHandler::builder()
            .filesystem(MemFs::new())
            .locksystem(FakeLs::new())
            .build_handler()
    }

    async fn resp_to_string(mut resp: http::Response<Body>) -> String {
        let mut data = Vec::new();
        let body = resp.body_mut();
        while let Some(chunk) = body.next().await {
            match chunk {
                Ok(bytes) => data.extend_from_slice(&bytes),
                Err(e) => panic!("Error reading body stream: {}", e),
            }
        }
        String::from_utf8(data).unwrap_or_else(|_| "".to_string())
    }

    #[tokio::test]
    async fn get_through_file_is_forbidden_or_not_found() {
        let server = setup();

        let put = server
            .handle(
                Request::builder()
                    .method("PUT")
                    .uri("/file")
                    .body(Body::from("hello"))
                    .unwrap(),
            )
            .await;
        assert_eq!(put.status(), StatusCode::CREATED);

        let get = server
            .handle(
                Request::builder()
                    .method("GET")
                    .uri("/file/anything")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert!(
            get.status() == StatusCode::FORBIDDEN || get.status() == StatusCode::NOT_FOUND,
            "{}",
            get.status()
        );
        let body = resp_to_string(get).await;
        assert_ne!(body, "hello");

        let file_get = server
            .handle(
                Request::builder()
                    .method("GET")
                    .uri("/file")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(file_get.status(), StatusCode::OK);
        assert_eq!(resp_to_string(file_get).await, "hello");
    }

    #[tokio::test]
    async fn partial_put_updates_etag() {
        let server = setup();

        let put = server
            .handle(
                Request::builder()
                    .method("PUT")
                    .uri("/file")
                    .header("X-OC-MTime", "1675789581")
                    .body(Body::from("hello"))
                    .unwrap(),
            )
            .await;
        assert_eq!(put.status(), StatusCode::CREATED);
        let etag = put
            .headers()
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let last_mod = put
            .headers()
            .get("last-modified")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        assert!(etag.is_some());
        assert_eq!(last_mod.as_deref(), Some("Tue, 07 Feb 2023 17:06:21 GMT"));

        let patch = server
            .handle(
                Request::builder()
                    .method("PATCH")
                    .uri("/file")
                    .header("Content-Type", "application/x-sabredav-partialupdate")
                    .header("X-Update-Range", "bytes=0-4")
                    .header("Content-Length", "5")
                    .body(Body::from("world"))
                    .unwrap(),
            )
            .await;
        assert!(patch.status().is_success(), "PATCH {}", patch.status());
        let patch_etag = patch
            .headers()
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let patch_mod = patch
            .headers()
            .get("last-modified")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        assert_ne!(patch_etag, etag, "etag should change after partial write");
        assert_ne!(
            patch_mod, last_mod,
            "last-modified should change after partial write"
        );
    }

    #[tokio::test]
    async fn move_file_onto_empty_collection_fails() {
        use bytes::Bytes;
        use dav_server::davpath::DavPath;
        use dav_server::fs::{DavFileSystem, FsError, OpenOptions};

        let fs = MemFs::new();
        let dir = DavPath::new("/dir").unwrap();
        let file = DavPath::new("/file").unwrap();
        fs.create_dir(&dir).await.unwrap();

        let oo = OpenOptions {
            write: true,
            create: true,
            truncate: true,
            ..OpenOptions::default()
        };
        let mut f = fs.open(&file, oo).await.unwrap();
        f.write_bytes(Bytes::from_static(b"hello")).await.unwrap();
        drop(f);

        let err = fs.rename(&file, &dir).await.unwrap_err();
        assert_eq!(err, FsError::Exists);

        let meta = fs.metadata(&dir).await.unwrap();
        assert!(meta.is_dir(), "/dir should still be a collection");
        let file_meta = fs.metadata(&file).await.unwrap();
        assert!(!file_meta.is_dir());
    }
}

#[cfg(feature = "memfs")]
mod partial_put_tests {
    use dav_server::{DavHandler, body::Body, fakels::FakeLs, memfs::MemFs};
    use http::{Request, StatusCode};

    const SABRE: &str = "application/x-sabredav-partialupdate";
    const BASE: &str = "1234567890";

    fn setup() -> DavHandler {
        DavHandler::builder()
            .filesystem(MemFs::new())
            .locksystem(FakeLs::new())
            .build_handler()
    }

    async fn resp_to_string(mut resp: http::Response<Body>) -> String {
        use futures_util::StreamExt;

        let mut data = Vec::new();
        let body = resp.body_mut();
        while let Some(chunk) = body.next().await {
            match chunk {
                Ok(bytes) => data.extend_from_slice(&bytes),
                Err(e) => panic!("Error reading body stream: {e}"),
            }
        }
        String::from_utf8(data).unwrap_or_default()
    }

    async fn put(server: &DavHandler, uri: &str, body: &str) -> StatusCode {
        server
            .handle(
                Request::builder()
                    .method("PUT")
                    .uri(uri)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .status()
    }

    async fn get_body(server: &DavHandler, uri: &str) -> (StatusCode, String) {
        let resp = server
            .handle(
                Request::builder()
                    .method("GET")
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        (resp.status(), resp_to_string(resp).await)
    }

    async fn patch(
        server: &DavHandler,
        uri: &str,
        range: &str,
        body: &str,
    ) -> http::Response<Body> {
        server
            .handle(
                Request::builder()
                    .method("PATCH")
                    .uri(uri)
                    .header("Content-Type", SABRE)
                    .header("X-Update-Range", range)
                    .header("Content-Length", body.len().to_string())
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
    }

    #[tokio::test]
    async fn patch_from_to_overwrites_inclusive_range() {
        let server = setup();
        assert_eq!(put(&server, "/file.txt", BASE).await, StatusCode::CREATED);
        let resp = patch(&server, "/file.txt", "bytes=0-3", "----").await;
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let (status, body) = get_body(&server, "/file.txt").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "----567890");
    }

    #[tokio::test]
    async fn patch_all_from_and_last_and_append() {
        let server = setup();
        assert_eq!(put(&server, "/from.txt", BASE).await, StatusCode::CREATED);
        let resp = patch(&server, "/from.txt", "bytes=2-", "----").await;
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert_eq!(get_body(&server, "/from.txt").await.1, "12----7890");

        assert_eq!(put(&server, "/last.txt", BASE).await, StatusCode::CREATED);
        let resp = patch(&server, "/last.txt", "bytes=-4", "----").await;
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert_eq!(get_body(&server, "/last.txt").await.1, "123456----");

        assert_eq!(put(&server, "/append.txt", BASE).await, StatusCode::CREATED);
        let resp = patch(&server, "/append.txt", "append", "----").await;
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert_eq!(get_body(&server, "/append.txt").await.1, "1234567890----");
    }

    #[tokio::test]
    async fn patch_error_statuses() {
        let server = setup();
        assert_eq!(put(&server, "/file.txt", BASE).await, StatusCode::CREATED);

        let wrong_type = server
            .handle(
                Request::builder()
                    .method("PATCH")
                    .uri("/file.txt")
                    .header("Content-Type", "text/plain")
                    .header("X-Update-Range", "bytes=0-3")
                    .header("Content-Length", "4")
                    .body(Body::from("----"))
                    .unwrap(),
            )
            .await;
        assert_eq!(wrong_type.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

        let no_length = server
            .handle(
                Request::builder()
                    .method("PATCH")
                    .uri("/file.txt")
                    .header("Content-Type", SABRE)
                    .header("X-Update-Range", "bytes=0-3")
                    .body(Body::from("----"))
                    .unwrap(),
            )
            .await;
        assert_eq!(no_length.status(), StatusCode::LENGTH_REQUIRED);

        let no_range = server
            .handle(
                Request::builder()
                    .method("PATCH")
                    .uri("/file.txt")
                    .header("Content-Type", SABRE)
                    .header("Content-Length", "4")
                    .body(Body::from("----"))
                    .unwrap(),
            )
            .await;
        assert_eq!(no_range.status(), StatusCode::BAD_REQUEST);

        let length_mismatch = patch(&server, "/file.txt", "bytes=0-3", "-----").await;
        assert_eq!(length_mismatch.status(), StatusCode::RANGE_NOT_SATISFIABLE);

        let inverted = patch(&server, "/file.txt", "bytes=6-3", "----").await;
        assert_eq!(inverted.status(), StatusCode::RANGE_NOT_SATISFIABLE);

        let last_too_long = patch(&server, "/file.txt", "bytes=-20", "----").await;
        assert_eq!(last_too_long.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    }

    #[tokio::test]
    async fn patch_on_collection_is_method_not_allowed() {
        let server = setup();
        let mkcol = server
            .handle(
                Request::builder()
                    .method("MKCOL")
                    .uri("/dir")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(mkcol.status(), StatusCode::CREATED);

        let resp = patch(&server, "/dir", "bytes=0-3", "----").await;
        assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn apache_put_content_range_updates_bytes() {
        let server = setup();
        assert_eq!(
            put(&server, "/file.txt", "hello world").await,
            StatusCode::CREATED
        );

        let resp = server
            .handle(
                Request::builder()
                    .method("PUT")
                    .uri("/file.txt")
                    .header("Content-Range", "bytes 3-6/*")
                    .header("Content-Length", "4")
                    .body(Body::from("ABCD"))
                    .unwrap(),
            )
            .await;
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert_eq!(get_body(&server, "/file.txt").await.1, "helABCDorld");
    }

    #[tokio::test]
    async fn apache_put_content_range_errors() {
        let server = setup();
        assert_eq!(put(&server, "/file.txt", BASE).await, StatusCode::CREATED);

        let inverted = server
            .handle(
                Request::builder()
                    .method("PUT")
                    .uri("/file.txt")
                    .header("Content-Range", "bytes 6-3/*")
                    .header("Content-Length", "4")
                    .body(Body::from("----"))
                    .unwrap(),
            )
            .await;
        assert_eq!(inverted.status(), StatusCode::BAD_REQUEST);

        let length_mismatch = server
            .handle(
                Request::builder()
                    .method("PUT")
                    .uri("/file.txt")
                    .header("Content-Range", "bytes 0-3/*")
                    .header("Content-Length", "5")
                    .body(Body::from("-----"))
                    .unwrap(),
            )
            .await;
        assert_eq!(length_mismatch.status(), StatusCode::RANGE_NOT_SATISFIABLE);

        let invalid = server
            .handle(
                Request::builder()
                    .method("PUT")
                    .uri("/file.txt")
                    .header("Content-Range", "not-a-range")
                    .body(Body::from("----"))
                    .unwrap(),
            )
            .await;
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    }
}

#[cfg(feature = "memfs")]
mod conditional_put_tests {
    use dav_server::{DavHandler, body::Body, fakels::FakeLs, memfs::MemFs};
    use http::{Request, StatusCode};

    fn setup() -> DavHandler {
        DavHandler::builder()
            .filesystem(MemFs::new())
            .locksystem(FakeLs::new())
            .build_handler()
    }

    async fn put_star(server: &DavHandler, uri: &str, body: &str, header_name: &str) -> StatusCode {
        server
            .handle(
                Request::builder()
                    .method("PUT")
                    .uri(uri)
                    .header(header_name, "*")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .status()
    }

    async fn get_body(server: &DavHandler, uri: &str) -> String {
        use futures_util::StreamExt;

        let mut resp = server
            .handle(
                Request::builder()
                    .method("GET")
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        let mut data = Vec::new();
        let body = resp.body_mut();
        while let Some(chunk) = body.next().await {
            match chunk {
                Ok(bytes) => data.extend_from_slice(&bytes),
                Err(e) => panic!("Error reading body stream: {e}"),
            }
        }
        String::from_utf8(data).unwrap_or_default()
    }

    #[tokio::test]
    async fn if_none_match_star_is_create_only() {
        let server = setup();
        assert_eq!(
            put_star(&server, "/file.txt", "v1", "If-None-Match").await,
            StatusCode::CREATED
        );
        assert_eq!(get_body(&server, "/file.txt").await, "v1");
        assert_eq!(
            put_star(&server, "/file.txt", "v2", "If-None-Match").await,
            StatusCode::PRECONDITION_FAILED
        );
        assert_eq!(get_body(&server, "/file.txt").await, "v1");
    }

    #[tokio::test]
    async fn if_match_star_is_update_only() {
        let server = setup();
        assert_eq!(
            put_star(&server, "/file.txt", "v1", "If-Match").await,
            StatusCode::PRECONDITION_FAILED
        );

        let created = server
            .handle(
                Request::builder()
                    .method("PUT")
                    .uri("/file.txt")
                    .body(Body::from("v1"))
                    .unwrap(),
            )
            .await;
        assert_eq!(created.status(), StatusCode::CREATED);

        assert_eq!(
            put_star(&server, "/file.txt", "v2", "If-Match").await,
            StatusCode::NO_CONTENT
        );
        assert_eq!(get_body(&server, "/file.txt").await, "v2");
    }

    #[tokio::test]
    async fn get_with_lone_quote_if_none_match_does_not_panic() {
        let server = setup();
        assert_eq!(
            put_star(&server, "/file.txt", "v1", "If-None-Match").await,
            StatusCode::CREATED
        );
        for header_name in ["If-None-Match", "If-Match", "If-Range"] {
            for value in ["\"", "W/\""] {
                let status = server
                    .handle(
                        Request::builder()
                            .method("GET")
                            .uri("/file.txt")
                            .header(header_name, value)
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .status();
                assert!(
                    matches!(
                        status,
                        StatusCode::OK | StatusCode::BAD_REQUEST | StatusCode::PRECONDITION_FAILED
                    ),
                    "{header_name}: {value} -> {status}"
                );
            }
        }
    }
}

#[cfg(feature = "memfs")]
mod options_method_tests {
    use dav_server::{DavHandler, DavMethodSet, body::Body, fakels::FakeLs, memfs::MemFs};
    use http::{Request, StatusCode};

    fn setup() -> DavHandler {
        DavHandler::builder()
            .filesystem(MemFs::new())
            .locksystem(FakeLs::new())
            .build_handler()
    }

    fn allow(resp: &http::Response<dav_server::body::Body>) -> String {
        resp.headers()
            .get("allow")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string()
    }

    fn has(allow: &str, method: &str) -> bool {
        allow.split(',').any(|m| m == method)
    }

    async fn options(server: &DavHandler, uri: &str) -> http::Response<dav_server::body::Body> {
        server
            .handle(
                Request::builder()
                    .method("OPTIONS")
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
    }

    #[tokio::test]
    async fn options_dav_advertises_sabredav_partialupdate() {
        let server = setup();
        let resp = options(&server, "/").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let dav = resp
            .headers()
            .get("DAV")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(dav.contains("sabredav-partialupdate"), "DAV header: {dav}");
        assert!(
            dav.contains("1,2,3") || dav.contains("1, 2, 3") || dav.starts_with("1,"),
            "{dav}"
        );
    }

    #[tokio::test]
    async fn options_allow_differs_for_unmapped_file_and_root() {
        let server = setup();

        let unmapped = options(&server, "/nope.txt").await;
        assert_eq!(unmapped.status(), StatusCode::OK);
        let a = allow(&unmapped);
        assert!(has(&a, "OPTIONS"), "{a}");
        assert!(has(&a, "MKCOL"), "{a}");
        assert!(has(&a, "PUT"), "{a}");
        assert!(has(&a, "LOCK"), "{a}");
        assert!(!has(&a, "GET"), "{a}");
        assert!(!has(&a, "DELETE"), "{a}");

        let put = server
            .handle(
                Request::builder()
                    .method("PUT")
                    .uri("/notes.txt")
                    .body(Body::from("hi"))
                    .unwrap(),
            )
            .await;
        assert_eq!(put.status(), StatusCode::CREATED);

        let file = options(&server, "/notes.txt").await;
        assert_eq!(file.status(), StatusCode::OK);
        let a = allow(&file);
        assert!(has(&a, "GET"), "{a}");
        assert!(has(&a, "HEAD"), "{a}");
        assert!(has(&a, "PUT"), "{a}");
        assert!(has(&a, "PATCH"), "{a}");
        assert!(has(&a, "DELETE"), "{a}");
        assert!(has(&a, "MOVE"), "{a}");
        assert!(!has(&a, "MKCOL"), "{a}");

        let root = options(&server, "/").await;
        assert_eq!(root.status(), StatusCode::OK);
        let a = allow(&root);
        assert!(has(&a, "OPTIONS"), "{a}");
        assert!(has(&a, "PROPFIND"), "{a}");
        assert!(has(&a, "COPY"), "{a}");
        assert!(!has(&a, "GET"), "{a}");
        assert!(!has(&a, "DELETE"), "{a}");
        assert!(!has(&a, "MOVE"), "{a}");
    }

    #[tokio::test]
    async fn methods_restrict_put_and_options_allow() {
        let server = DavHandler::builder()
            .filesystem(MemFs::new())
            .locksystem(FakeLs::new())
            .methods(DavMethodSet::HTTP_RO)
            .build_handler();

        let put = server
            .handle(
                Request::builder()
                    .method("PUT")
                    .uri("/notes.txt")
                    .body(Body::from("hi"))
                    .unwrap(),
            )
            .await;
        assert_eq!(put.status(), StatusCode::METHOD_NOT_ALLOWED);

        let resp = options(&server, "/").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let a = allow(&resp);
        assert!(has(&a, "OPTIONS"), "{a}");
        assert!(!has(&a, "PUT"), "{a}");
        assert!(!has(&a, "LOCK"), "{a}");
        assert!(!has(&a, "PROPFIND"), "{a}");
    }
}
