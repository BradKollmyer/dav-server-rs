#[cfg(all(feature = "caldav", feature = "memfs"))]
mod caldav_tests {
    use dav_server::{DavHandler, body::Body, caldav::*, fakels::FakeLs};
    use http::response::Response;
    use http::{Method, Request, StatusCode};

    async fn mkcol(server: &DavHandler, uri: &str) {
        let req = Request::builder()
            .method("MKCOL")
            .uri(uri)
            .body(Body::empty())
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::CREATED, "MKCOL {uri}");
    }

    async fn setup_caldav_server() -> DavHandler {
        let server = DavHandler::builder()
            .filesystem(dav_server::memfs::MemFs::new())
            // .filesystem(dav_server::localfs::LocalFs::new("/tmp", true, false, false))
            .locksystem(FakeLs::new())
            .build_handler();
        mkcol(&server, "/calendars").await;
        server
    }

    async fn setup_caldav_server2() -> DavHandler {
        let server = setup_caldav_server().await;

        let req = Request::builder()
            .method("MKCALENDAR")
            .uri("/calendars/my-calendar")
            .body(Body::empty())
            .unwrap();
        server.handle(req).await;

        server
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

    fn create_ics_data(uid: &str, summary: &str) -> String {
        create_vevent_ics(uid, summary, "20240101T120000Z", "20240101T130000Z")
    }

    fn create_vevent_ics(uid: &str, summary: &str, dtstart: &str, dtend: &str) -> String {
        format!(
            "BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//Test//Test//EN
BEGIN:VEVENT
UID:{uid}
DTSTART:{dtstart}
DTEND:{dtend}
SUMMARY:{summary}
DESCRIPTION:This is a test event
END:VEVENT
END:VCALENDAR"
        )
    }

    fn create_vtodo_ics(uid: &str, summary: &str) -> String {
        format!(
            "BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//Test//Test//EN
BEGIN:VTODO
UID:{uid}
DTSTART:20240101T120000Z
DUE:20240101T130000Z
SUMMARY:{summary}
END:VTODO
END:VCALENDAR"
        )
    }

    async fn report_calendar_query(server: &DavHandler, body: &str) -> (StatusCode, String) {
        let req = Request::builder()
            .method("REPORT")
            .uri("/calendars/my-calendar")
            .header("Depth", "1")
            .body(Body::from(body.to_string()))
            .unwrap();
        let resp = server.handle(req).await;
        let status = resp.status();
        let body_str = resp_to_string(resp).await;
        (status, body_str)
    }

    async fn put_ics_data(server: &DavHandler, ics_data: String, uri: &str) -> Response<Body> {
        let req = Request::builder()
            .method(Method::PUT)
            .uri(uri)
            .header("Content-Type", "text/calendar")
            .body(Body::from(ics_data))
            .unwrap();
        server.handle(req).await
    }

    #[tokio::test]
    async fn test_caldav_options() {
        let server = setup_caldav_server().await;

        let req = Request::builder()
            .method(Method::OPTIONS)
            .uri("/")
            .body(Body::empty())
            .unwrap();

        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let dav_header = resp.headers().get("DAV").unwrap();
        let dav_str = dav_header.to_str().unwrap();
        assert!(dav_str.contains("calendar-access"));
    }

    #[tokio::test]
    async fn test_mkcalendar() {
        let server = setup_caldav_server().await;

        let req = Request::builder()
            .method("MKCALENDAR")
            .uri("/calendars/my-calendar")
            .body(Body::empty())
            .unwrap();

        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn test_mkcalendar_outside_calendars_prefix_is_calendar() {
        let server = setup_caldav_server().await;

        let req = Request::builder()
            .method("MKCOL")
            .uri("/other")
            .body(Body::empty())
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::CREATED);

        let req = Request::builder()
            .method("MKCALENDAR")
            .uri("/other/cal")
            .body(Body::empty())
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::CREATED);

        let propfind_body = r#"<?xml version="1.0" encoding="utf-8" ?>
<D:propfind xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop>
    <D:resourcetype/>
  </D:prop>
</D:propfind>"#;

        let req = Request::builder()
            .method("PROPFIND")
            .uri("/other/cal")
            .header("Depth", "0")
            .body(Body::from(propfind_body))
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::MULTI_STATUS);
        let body_str = resp_to_string(resp).await;
        assert!(
            body_str.contains("<C:calendar"),
            "MKCALENDAR outside /calendars must still be a calendar: {body_str}"
        );
    }

    #[tokio::test]
    async fn test_mkcol_under_calendars_is_not_calendar() {
        let server = setup_caldav_server().await;

        let req = Request::builder()
            .method("MKCOL")
            .uri("/calendars/not-a-cal")
            .body(Body::empty())
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::CREATED);

        let propfind_body = r#"<?xml version="1.0" encoding="utf-8" ?>
<D:propfind xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop>
    <D:resourcetype/>
  </D:prop>
</D:propfind>"#;

        let req = Request::builder()
            .method("PROPFIND")
            .uri("/calendars/not-a-cal")
            .header("Depth", "0")
            .body(Body::from(propfind_body))
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::MULTI_STATUS);
        let body_str = resp_to_string(resp).await;
        assert!(
            body_str.contains("collection"),
            "MKCOL under /calendars should be a collection: {body_str}"
        );
        assert!(
            !body_str.contains("<C:calendar"),
            "MKCOL under /calendars must not be a calendar: {body_str}"
        );
    }

    #[tokio::test]
    async fn test_mkcol_garbage_body_is_unsupported_media_type() {
        let server = setup_caldav_server().await;

        let req = Request::builder()
            .method("MKCOL")
            .uri("/garbage-col")
            .header("Content-Type", "xzy-foo/bar-512")
            .body(Body::from("afafafaf"))
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

        let req = Request::builder()
            .method("MKCOL")
            .uri("/wrong-root-col")
            .body(Body::from(
                r#"<?xml version="1.0"?><D:propfind xmlns:D="DAV:"/>"#,
            ))
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }

    #[tokio::test]
    async fn test_mkcalendar_already_exists() {
        // First create a regular collection
        let server = setup_caldav_server2().await;

        // Try to create calendar collection on existing path
        let req = Request::builder()
            .method("MKCALENDAR")
            .uri("/calendars/my-calendar")
            .body(Body::empty())
            .unwrap();

        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn test_mkcalendar_missing_parent_is_conflict() {
        let server = setup_caldav_server().await;

        let req = Request::builder()
            .method("MKCALENDAR")
            .uri("/no-such-parent/calendar")
            .body(Body::empty())
            .unwrap();

        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn test_mkcalendar_garbage_body_is_bad_request() {
        let server = setup_caldav_server().await;

        let req = Request::builder()
            .method("MKCALENDAR")
            .uri("/calendars/garbage-cal")
            .body(Body::from("this is not xml"))
            .unwrap();

        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_mkcalendar_sets_displayname() {
        let server = setup_caldav_server().await;

        let mkcalendar_body = r#"<?xml version="1.0" encoding="utf-8" ?>
<C:mkcalendar xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:set>
    <D:prop>
      <D:displayname>Lisa's Events</D:displayname>
    </D:prop>
  </D:set>
</C:mkcalendar>"#;

        let req = Request::builder()
            .method("MKCALENDAR")
            .uri("/calendars/named-calendar")
            .body(Body::from(mkcalendar_body.to_string()))
            .unwrap();

        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::CREATED);

        let propfind_body = r#"<?xml version="1.0" encoding="utf-8" ?>
<D:propfind xmlns:D="DAV:">
  <D:prop>
    <D:displayname/>
  </D:prop>
</D:propfind>"#;

        let req = Request::builder()
            .method("PROPFIND")
            .uri("/calendars/named-calendar")
            .header("Depth", "0")
            .body(Body::from(propfind_body))
            .unwrap();

        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::MULTI_STATUS);

        let body_str = resp_to_string(resp).await;
        assert!(
            body_str.contains("Lisa's Events"),
            "displayname missing: {body_str}"
        );
    }

    #[tokio::test]
    async fn test_calendar_propfind() {
        let server = setup_caldav_server2().await;

        // PROPFIND request
        let propfind_body = r#"<?xml version="1.0" encoding="utf-8" ?>
<D:propfind xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop>
    <D:resourcetype/>
    <D:getetag/>
    <D:supported-report-set/>
    <C:supported-calendar-component-set/>
    <C:supported-calendar-data/>
  </D:prop>
</D:propfind>"#;

        let req = Request::builder()
            .method("PROPFIND")
            .uri("/calendars/my-calendar")
            .header("Depth", "0")
            .body(Body::from(propfind_body))
            .unwrap();

        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::MULTI_STATUS);

        // Check that response contains CalDAV properties
        let body_str = resp_to_string(resp).await;
        println!("{}", body_str);
        assert!(body_str.contains("resourcetype"));
        assert!(
            body_str.contains("<C:calendar"),
            "MKCALENDAR collection must have calendar resourcetype: {body_str}"
        );
        assert!(body_str.contains("getetag"));
        assert!(body_str.contains("supported-report-set"));
        assert!(body_str.contains("supported-calendar-component-set"));
        assert!(body_str.contains("supported-calendar-data"));
    }

    #[tokio::test]
    async fn test_nested_path_is_not_calendar_collection() {
        let server = setup_caldav_server2().await;

        let req = Request::builder()
            .method("MKCOL")
            .uri("/calendars/my-calendar/nested")
            .body(Body::empty())
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::CREATED);

        let propfind_body = r#"<?xml version="1.0" encoding="utf-8" ?>
<D:propfind xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop>
    <D:resourcetype/>
  </D:prop>
</D:propfind>"#;

        let req = Request::builder()
            .method("PROPFIND")
            .uri("/calendars/my-calendar/nested")
            .header("Depth", "0")
            .body(Body::from(propfind_body))
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::MULTI_STATUS);
        let body_str = resp_to_string(resp).await;
        assert!(
            body_str.contains("collection"),
            "nested dir should remain a collection: {body_str}"
        );
        assert!(
            !body_str.contains("<C:calendar"),
            "nested dir must not be a calendar collection: {body_str}"
        );

        let ics_data = create_ics_data("test-event-1", "Test Event");
        put_ics_data(&server, ics_data, "/calendars/my-calendar/event.ics").await;
        let req = Request::builder()
            .method("PROPFIND")
            .uri("/calendars/my-calendar/event.ics")
            .header("Depth", "0")
            .body(Body::from(propfind_body))
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::MULTI_STATUS);
        let body_str = resp_to_string(resp).await;
        assert!(
            !body_str.contains("<C:calendar"),
            "calendar object must not be a calendar collection: {body_str}"
        );
    }

    #[tokio::test]
    async fn test_supported_report_set_on_calendar() {
        let server = setup_caldav_server2().await;

        let propfind_body = r#"<?xml version="1.0" encoding="utf-8" ?>
<D:propfind xmlns:D="DAV:">
  <D:prop>
    <D:supported-report-set/>
  </D:prop>
</D:propfind>"#;

        let req = Request::builder()
            .method("PROPFIND")
            .uri("/calendars/my-calendar")
            .header("Depth", "0")
            .body(Body::from(propfind_body))
            .unwrap();

        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::MULTI_STATUS);
        let body_str = resp_to_string(resp).await;
        assert!(
            body_str.contains("calendar-query"),
            "calendar collection should advertise calendar-query: {body_str}"
        );
        assert!(body_str.contains("calendar-multiget"));
        assert!(
            body_str.contains("free-busy-query"),
            "calendar collection should advertise free-busy-query: {body_str}"
        );
        assert!(!body_str.contains("addressbook-query"));
    }

    #[tokio::test]
    async fn test_calendar_event_put() {
        let server = setup_caldav_server2().await;

        // PUT a calendar event
        let ics_data = create_ics_data("test-event-1", "Test Event");
        let resp = put_ics_data(&server, ics_data, "/calendars/my-calendar/event.ics").await;
        assert!(resp.status().is_success());
    }

    #[tokio::test]
    async fn test_calendar_put_invalid_ics() {
        let server = setup_caldav_server2().await;

        let resp = put_ics_data(
            &server,
            "this is not iCalendar".to_string(),
            "/calendars/my-calendar/bad.ics",
        )
        .await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);

        let req = Request::builder()
            .method(Method::GET)
            .uri("/calendars/my-calendar/bad.ics")
            .body(Body::empty())
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        let ics_data = create_ics_data("valid-event", "Valid Event");
        let resp = put_ics_data(&server, ics_data, "/calendars/my-calendar/event.ics").await;
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn test_calendar_partial_put_invalid_restores() {
        let server = setup_caldav_server2().await;
        let ics_data = create_ics_data("test-event-1", "Test Event");
        let resp = put_ics_data(
            &server,
            ics_data.clone(),
            "/calendars/my-calendar/event.ics",
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CREATED);

        let resp = server
            .handle(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/calendars/my-calendar/event.ics")
                    .header("Content-Range", "bytes 0-4/*")
                    .header("Content-Length", "5")
                    .body(Body::from("XXXXX"))
                    .unwrap(),
            )
            .await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);

        let req = Request::builder()
            .method(Method::GET)
            .uri("/calendars/my-calendar/event.ics")
            .body(Body::empty())
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp_to_string(resp).await, ics_data);
    }

    #[tokio::test]
    async fn test_calendar_patch_invalid_restores() {
        let server = setup_caldav_server2().await;
        let ics_data = create_ics_data("test-event-1", "Test Event");
        let resp = put_ics_data(
            &server,
            ics_data.clone(),
            "/calendars/my-calendar/event.ics",
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CREATED);

        let resp = server
            .handle(
                Request::builder()
                    .method("PATCH")
                    .uri("/calendars/my-calendar/event.ics")
                    .header("Content-Type", "application/x-sabredav-partialupdate")
                    .header("X-Update-Range", "bytes=0-4")
                    .header("Content-Length", "5")
                    .body(Body::from("XXXXX"))
                    .unwrap(),
            )
            .await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);

        let req = Request::builder()
            .method(Method::GET)
            .uri("/calendars/my-calendar/event.ics")
            .body(Body::empty())
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp_to_string(resp).await, ics_data);
    }

    #[tokio::test]
    async fn test_calendar_patch_premature_eof_restores() {
        let server = setup_caldav_server2().await;
        let ics_data = create_ics_data("test-event-1", "Test Event");
        let resp = put_ics_data(
            &server,
            ics_data.clone(),
            "/calendars/my-calendar/event.ics",
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CREATED);

        // Body is shorter than the announced length: the client went away.
        let resp = server
            .handle(
                Request::builder()
                    .method("PATCH")
                    .uri("/calendars/my-calendar/event.ics")
                    .header("Content-Type", "application/x-sabredav-partialupdate")
                    .header("X-Update-Range", "bytes=0-9")
                    .header("Content-Length", "10")
                    .body(Body::from("XXXXX"))
                    .unwrap(),
            )
            .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let req = Request::builder()
            .method(Method::GET)
            .uri("/calendars/my-calendar/event.ics")
            .body(Body::empty())
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp_to_string(resp).await, ics_data);
    }

    #[tokio::test]
    async fn test_calendar_put_premature_eof_removes_new_object() {
        let server = setup_caldav_server2().await;
        let ics_data = create_ics_data("test-event-1", "Test Event");
        let partial = ics_data[..ics_data.len() / 2].to_string();

        let resp = server
            .handle(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/calendars/my-calendar/event.ics")
                    .header("Content-Type", "text/calendar")
                    .header("Content-Length", ics_data.len().to_string())
                    .body(Body::from(partial))
                    .unwrap(),
            )
            .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let req = Request::builder()
            .method(Method::GET)
            .uri("/calendars/my-calendar/event.ics")
            .body(Body::empty())
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_calendar_put_max_resource_size() {
        let server = setup_caldav_server2().await;

        let small = create_ics_data("small-event", "Small Event");
        let resp = put_ics_data(&server, small, "/calendars/my-calendar/small.ics").await;
        assert_eq!(resp.status(), StatusCode::CREATED);

        let mut big = create_ics_data("big-event", "Big Event");
        big.push_str(&"X".repeat(DEFAULT_MAX_RESOURCE_SIZE as usize + 1));
        let resp = put_ics_data(&server, big, "/calendars/my-calendar/big.ics").await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_calendar_query_report() {
        let server = setup_caldav_server2().await;
        let ics_data = create_ics_data("test-event-1", "Test Event");
        put_ics_data(&server, ics_data, "/calendars/my-calendar/event.ics").await;

        // REPORT calendar-query
        let report_body = r#"<?xml version="1.0" encoding="utf-8" ?>
<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop>
    <C:calendar-data/>
  </D:prop>
  <C:filter>
    <C:comp-filter name="VCALENDAR">
      <C:comp-filter name="VEVENT"/>
    </C:comp-filter>
  </C:filter>
</C:calendar-query>"#;

        let req = Request::builder()
            .method("REPORT")
            .uri("/calendars/my-calendar")
            .header("Depth", "1")
            .body(Body::from(report_body))
            .unwrap();

        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::MULTI_STATUS);

        let body_str = resp_to_string(resp).await;
        assert!(body_str.contains("calendar-data"));
        assert!(body_str.contains("Test Event"));
    }

    // LocalFs rather than MemFs: MemFs::create_dir happily "creates" the root.
    #[cfg(feature = "localfs")]
    #[tokio::test]
    async fn test_mkcalendar_on_prefixed_root_is_rejected_without_panic() {
        use std::time::{SystemTime, UNIX_EPOCH};

        let dir = std::env::temp_dir().join(format!(
            "dav-mkcalendar-root-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let server = DavHandler::builder()
            .filesystem(dav_server::localfs::LocalFs::new(&dir, true, false, false))
            .locksystem(FakeLs::new())
            .strip_prefix("/dav")
            .build_handler();

        // The parent of the target is looked up before create_dir rejects
        // the root; this used to panic on the prefixed root.
        for uri in ["/dav/", "/dav"] {
            let req = Request::builder()
                .method("MKCALENDAR")
                .uri(uri)
                .body(Body::empty())
                .unwrap();
            let resp = server.handle(req).await;
            assert!(
                resp.status().is_client_error(),
                "MKCALENDAR {uri}: {}",
                resp.status()
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_calendar_query_report_hrefs_include_strip_prefix() {
        let server = DavHandler::builder()
            .filesystem(dav_server::memfs::MemFs::new())
            .locksystem(FakeLs::new())
            .strip_prefix("/dav")
            .build_handler();

        mkcol(&server, "/dav/calendars").await;

        let req = Request::builder()
            .method("MKCALENDAR")
            .uri("/dav/calendars/my-calendar")
            .body(Body::empty())
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::CREATED);

        let ics_data = create_ics_data("test-event-1", "Test Event");
        let resp = put_ics_data(&server, ics_data, "/dav/calendars/my-calendar/event.ics").await;
        assert!(resp.status().is_success());

        let report_body = r#"<?xml version="1.0" encoding="utf-8" ?>
<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop>
    <C:calendar-data/>
  </D:prop>
  <C:filter>
    <C:comp-filter name="VCALENDAR">
      <C:comp-filter name="VEVENT"/>
    </C:comp-filter>
  </C:filter>
</C:calendar-query>"#;

        let req = Request::builder()
            .method("REPORT")
            .uri("/dav/calendars/my-calendar")
            .header("Depth", "1")
            .body(Body::from(report_body))
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::MULTI_STATUS);

        let body_str = resp_to_string(resp).await;
        assert!(
            body_str.contains("/dav/calendars/my-calendar/event.ics"),
            "REPORT href missing mount prefix: {body_str}"
        );
        assert!(
            body_str.contains("Test Event"),
            "calendar-data missing event: {body_str}"
        );
    }

    #[tokio::test]
    async fn test_calendar_query_filters_by_component_type() {
        let server = setup_caldav_server2().await;
        put_ics_data(
            &server,
            create_ics_data("event-1", "Test Event"),
            "/calendars/my-calendar/event.ics",
        )
        .await;
        put_ics_data(
            &server,
            create_vtodo_ics("todo-1", "Test Todo"),
            "/calendars/my-calendar/todo.ics",
        )
        .await;

        let event_query = r#"<?xml version="1.0" encoding="utf-8" ?>
<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop>
    <C:calendar-data/>
  </D:prop>
  <C:filter>
    <C:comp-filter name="VCALENDAR">
      <C:comp-filter name="VEVENT"/>
    </C:comp-filter>
  </C:filter>
</C:calendar-query>"#;

        let (status, body) = report_calendar_query(&server, event_query).await;
        assert_eq!(status, StatusCode::MULTI_STATUS);
        assert!(
            body.contains("Test Event"),
            "VEVENT query missing event: {body}"
        );
        assert!(
            !body.contains("Test Todo"),
            "VEVENT query must not return VTODO: {body}"
        );

        let todo_query = r#"<?xml version="1.0" encoding="utf-8" ?>
<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop>
    <C:calendar-data/>
  </D:prop>
  <C:filter>
    <C:comp-filter name="VCALENDAR">
      <C:comp-filter name="VTODO"/>
    </C:comp-filter>
  </C:filter>
</C:calendar-query>"#;

        let (status, body) = report_calendar_query(&server, todo_query).await;
        assert_eq!(status, StatusCode::MULTI_STATUS);
        assert!(
            body.contains("Test Todo"),
            "VTODO query missing todo: {body}"
        );
        assert!(
            !body.contains("Test Event"),
            "VTODO query must not return VEVENT: {body}"
        );
    }

    #[tokio::test]
    async fn test_calendar_query_filters_by_summary_text_match() {
        let server = setup_caldav_server2().await;
        put_ics_data(
            &server,
            create_ics_data("meet-1", "Team Meeting"),
            "/calendars/my-calendar/meeting.ics",
        )
        .await;
        put_ics_data(
            &server,
            create_ics_data("appt-1", "Doctor Appointment"),
            "/calendars/my-calendar/appointment.ics",
        )
        .await;

        let summary_query = r#"<?xml version="1.0" encoding="utf-8" ?>
<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop>
    <C:calendar-data/>
  </D:prop>
  <C:filter>
    <C:comp-filter name="VCALENDAR">
      <C:comp-filter name="VEVENT">
        <C:prop-filter name="SUMMARY">
          <C:text-match collation="i;unicode-casemap">meeting</C:text-match>
        </C:prop-filter>
      </C:comp-filter>
    </C:comp-filter>
  </C:filter>
</C:calendar-query>"#;

        let (status, body) = report_calendar_query(&server, summary_query).await;
        assert_eq!(status, StatusCode::MULTI_STATUS);
        assert!(
            body.contains("Team Meeting"),
            "SUMMARY text-match missing meeting: {body}"
        );
        assert!(
            !body.contains("Doctor Appointment"),
            "SUMMARY text-match must not return appointment: {body}"
        );
    }

    #[tokio::test]
    async fn test_calendar_query_without_filter_is_bad_request() {
        let server = setup_caldav_server2().await;
        put_ics_data(
            &server,
            create_ics_data("event-1", "Test Event"),
            "/calendars/my-calendar/event.ics",
        )
        .await;

        let no_filter = r#"<?xml version="1.0" encoding="utf-8" ?>
<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop>
    <C:calendar-data/>
  </D:prop>
</C:calendar-query>"#;

        let (status, _) = report_calendar_query(&server, no_filter).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let empty_filter = r#"<?xml version="1.0" encoding="utf-8" ?>
<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop>
    <C:calendar-data/>
  </D:prop>
  <C:filter/>
</C:calendar-query>"#;

        let (status, _) = report_calendar_query(&server, empty_filter).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_calendar_query_time_range_excludes_outside_event() {
        let server = setup_caldav_server2().await;
        put_ics_data(
            &server,
            create_vevent_ics(
                "jan-event",
                "January Event",
                "20240101T120000Z",
                "20240101T130000Z",
            ),
            "/calendars/my-calendar/jan.ics",
        )
        .await;
        put_ics_data(
            &server,
            create_vevent_ics(
                "jun-event",
                "June Event",
                "20240615T120000Z",
                "20240615T130000Z",
            ),
            "/calendars/my-calendar/jun.ics",
        )
        .await;

        let june_query = r#"<?xml version="1.0" encoding="utf-8" ?>
<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop>
    <C:calendar-data/>
  </D:prop>
  <C:filter>
    <C:comp-filter name="VCALENDAR">
      <C:comp-filter name="VEVENT">
        <C:time-range start="20240601T000000Z" end="20240701T000000Z"/>
      </C:comp-filter>
    </C:comp-filter>
  </C:filter>
</C:calendar-query>"#;

        let (status, body) = report_calendar_query(&server, june_query).await;
        assert_eq!(status, StatusCode::MULTI_STATUS);
        assert!(
            body.contains("June Event"),
            "in-range event missing: {body}"
        );
        assert!(
            !body.contains("January Event"),
            "out-of-range event must be excluded: {body}"
        );
    }

    #[tokio::test]
    async fn test_calendar_query_prop_filter_time_range_on_dtstart() {
        let server = setup_caldav_server2().await;
        put_ics_data(
            &server,
            create_vevent_ics(
                "jan-event",
                "January Event",
                "20240101T120000Z",
                "20240101T130000Z",
            ),
            "/calendars/my-calendar/jan.ics",
        )
        .await;
        put_ics_data(
            &server,
            create_vevent_ics(
                "jun-event",
                "June Event",
                "20240615T120000Z",
                "20240615T130000Z",
            ),
            "/calendars/my-calendar/jun.ics",
        )
        .await;

        let june_query = r#"<?xml version="1.0" encoding="utf-8" ?>
<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop>
    <C:calendar-data/>
  </D:prop>
  <C:filter>
    <C:comp-filter name="VCALENDAR">
      <C:comp-filter name="VEVENT">
        <C:prop-filter name="DTSTART">
          <C:time-range start="20240601T000000Z" end="20240701T000000Z"/>
        </C:prop-filter>
      </C:comp-filter>
    </C:comp-filter>
  </C:filter>
</C:calendar-query>"#;

        let (status, body) = report_calendar_query(&server, june_query).await;
        assert_eq!(status, StatusCode::MULTI_STATUS);
        assert!(
            body.contains("June Event"),
            "DTSTART in-range event missing: {body}"
        );
        assert!(
            !body.contains("January Event"),
            "DTSTART out-of-range event must be excluded: {body}"
        );
    }

    #[tokio::test]
    async fn test_calendar_query_param_filter_on_attendee_partstat() {
        let server = setup_caldav_server2().await;
        put_ics_data(
            &server,
            "BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//Test//Test//EN
BEGIN:VEVENT
UID:needs-1
DTSTART:20240615T120000Z
DTEND:20240615T130000Z
SUMMARY:Needs Action
ATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:a@example.com
END:VEVENT
END:VCALENDAR"
                .to_string(),
            "/calendars/my-calendar/needs.ics",
        )
        .await;
        put_ics_data(
            &server,
            "BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//Test//Test//EN
BEGIN:VEVENT
UID:accepted-1
DTSTART:20240615T120000Z
DTEND:20240615T130000Z
SUMMARY:Accepted
ATTENDEE;PARTSTAT=ACCEPTED:mailto:b@example.com
END:VEVENT
END:VCALENDAR"
                .to_string(),
            "/calendars/my-calendar/accepted.ics",
        )
        .await;

        let partstat_query = r#"<?xml version="1.0" encoding="utf-8" ?>
<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop>
    <C:calendar-data/>
  </D:prop>
  <C:filter>
    <C:comp-filter name="VCALENDAR">
      <C:comp-filter name="VEVENT">
        <C:prop-filter name="ATTENDEE">
          <C:param-filter name="PARTSTAT">
            <C:text-match collation="i;unicode-casemap">NEEDS-ACTION</C:text-match>
          </C:param-filter>
        </C:prop-filter>
      </C:comp-filter>
    </C:comp-filter>
  </C:filter>
</C:calendar-query>"#;

        let (status, body) = report_calendar_query(&server, partstat_query).await;
        assert_eq!(status, StatusCode::MULTI_STATUS);
        assert!(
            body.contains("Needs Action"),
            "PARTSTAT NEEDS-ACTION event missing: {body}"
        );
        assert!(
            !body.contains("Accepted"),
            "PARTSTAT ACCEPTED event must be excluded: {body}"
        );
    }

    #[tokio::test]
    async fn test_calendar_multiget_report() {
        let server = setup_caldav_server2().await;
        let ics_data = create_ics_data("test-event-1", "Test Event0001");
        let resp = put_ics_data(&server, ics_data, "/calendars/my-calendar/event1.ics").await;
        assert_eq!(resp.status(), StatusCode::CREATED);
        let ics_data = create_ics_data("test-event-2", "Test Event2222");
        let resp = put_ics_data(&server, ics_data, "/calendars/my-calendar/event2.ics").await;
        assert_eq!(resp.status(), StatusCode::CREATED);

        // REPORT calendar-multiget
        let report_body = r#"<?xml version="1.0" encoding="utf-8" ?>
<C:calendar-multiget xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop>
    <C:calendar-data/>
  </D:prop>
  <D:href>/calendars/my-calendar/event1.ics</D:href>
  <D:href>/calendars/my-calendar/event2.ics</D:href>
</C:calendar-multiget>"#;

        let req = Request::builder()
            .method("REPORT")
            .uri("/calendars/my-calendar")
            .body(Body::from(report_body))
            .unwrap();

        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::MULTI_STATUS);

        let body_str = resp_to_string(resp).await;
        assert!(
            body_str.contains("calendar-data"),
            "Response body missing 'calendar-data': {}",
            body_str
        );
        assert!(
            body_str.contains("Test Event0001"),
            "Response body missing 'Test Event0001': {}",
            body_str
        );
        assert!(
            body_str.contains("Test Event2222"),
            "Response body missing 'Test Event2222': {}",
            body_str
        );
    }

    #[tokio::test]
    async fn test_calendar_multiget_confines_hrefs_to_collection() {
        let server = setup_caldav_server2().await;
        let inside = create_ics_data("inside-event", "Inside Event");
        let resp = put_ics_data(&server, inside, "/calendars/my-calendar/event.ics").await;
        assert_eq!(resp.status(), StatusCode::CREATED);

        let outside = create_ics_data("outside-event", "Outside Event");
        let resp = put_ics_data(&server, outside, "/other.ics").await;
        assert_eq!(resp.status(), StatusCode::CREATED);

        let report_body = r#"<?xml version="1.0" encoding="utf-8" ?>
<C:calendar-multiget xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop>
    <C:calendar-data/>
  </D:prop>
  <D:href>/calendars/my-calendar/event.ics</D:href>
  <D:href>/other.ics</D:href>
</C:calendar-multiget>"#;

        let req = Request::builder()
            .method("REPORT")
            .uri("/calendars/my-calendar")
            .body(Body::from(report_body))
            .unwrap();

        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::MULTI_STATUS);

        let body_str = resp_to_string(resp).await;
        assert!(
            body_str.contains("Inside Event"),
            "in-collection event missing: {body_str}"
        );
        assert!(
            !body_str.contains("Outside Event"),
            "outsider calendar-data must not be returned: {body_str}"
        );
        assert!(
            body_str.contains("/other.ics"),
            "outsider href missing from 207: {body_str}"
        );
        assert!(
            body_str.contains("404 Not Found"),
            "outsider href must be 404: {body_str}"
        );
    }

    #[tokio::test]
    async fn test_calendar_multiget_on_object_resource() {
        let server = setup_caldav_server2().await;
        let ics_data = create_ics_data("self-event", "Self Event");
        let resp = put_ics_data(&server, ics_data, "/calendars/my-calendar/event.ics").await;
        assert_eq!(resp.status(), StatusCode::CREATED);

        // RFC 4791 7.9: the REPORT may be addressed to the calendar object
        // resource itself, listing its own href.
        let report_body = r#"<?xml version="1.0" encoding="utf-8" ?>
<C:calendar-multiget xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop>
    <C:calendar-data/>
  </D:prop>
  <D:href>/calendars/my-calendar/event.ics</D:href>
</C:calendar-multiget>"#;

        let req = Request::builder()
            .method("REPORT")
            .uri("/calendars/my-calendar/event.ics")
            .body(Body::from(report_body))
            .unwrap();

        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::MULTI_STATUS);

        let body_str = resp_to_string(resp).await;
        assert!(
            body_str.contains("HTTP/1.1 200 OK"),
            "object href must be 200: {body_str}"
        );
        assert!(
            !body_str.contains("404 Not Found"),
            "object href must not be 404: {body_str}"
        );
        assert!(
            body_str.contains("Self Event"),
            "calendar-data missing: {body_str}"
        );
    }

    /// A calendar with a visible and a dot-prefixed event, served by a
    /// handler that hides dot-prefixed names. The hidden file is created
    /// through a second handler on the same MemFs that does not hide it.
    async fn setup_hidden_calendar_server() -> DavHandler {
        let fs = dav_server::memfs::MemFs::new();
        let writer = DavHandler::builder()
            .filesystem(fs.clone())
            .locksystem(FakeLs::new())
            .build_handler();
        mkcol(&writer, "/calendars").await;
        for uri in ["/calendars/my-calendar", "/calendars/.hidden-cal"] {
            let req = Request::builder()
                .method("MKCALENDAR")
                .uri(uri)
                .body(Body::empty())
                .unwrap();
            let resp = writer.handle(req).await;
            assert_eq!(resp.status(), StatusCode::CREATED, "MKCALENDAR {uri}");
        }
        let ics = create_ics_data("visible-event", "Visible Event");
        let resp = put_ics_data(&writer, ics, "/calendars/my-calendar/visible.ics").await;
        assert_eq!(resp.status(), StatusCode::CREATED);
        let ics = create_ics_data("hidden-event", "Hidden Event");
        let resp = put_ics_data(&writer, ics, "/calendars/my-calendar/.hidden.ics").await;
        assert_eq!(resp.status(), StatusCode::CREATED);

        DavHandler::builder()
            .filesystem(fs)
            .locksystem(FakeLs::new())
            .hide_dot_prefix(dav_server::DavOptionHide::Always)
            .build_handler()
    }

    #[tokio::test]
    async fn test_calendar_query_skips_hidden_entries() {
        let server = setup_hidden_calendar_server().await;

        let report_body = r#"<?xml version="1.0" encoding="utf-8" ?>
<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop>
    <C:calendar-data/>
  </D:prop>
  <C:filter>
    <C:comp-filter name="VCALENDAR"/>
  </C:filter>
</C:calendar-query>"#;

        let (status, body_str) = report_calendar_query(&server, report_body).await;
        assert_eq!(status, StatusCode::MULTI_STATUS);
        assert!(
            body_str.contains("Visible Event"),
            "visible event missing: {body_str}"
        );
        assert!(
            !body_str.contains(".hidden.ics") && !body_str.contains("Hidden Event"),
            "hidden event must not be listed: {body_str}"
        );
    }

    #[tokio::test]
    async fn test_calendar_multiget_hidden_href_is_404() {
        let server = setup_hidden_calendar_server().await;

        let report_body = r#"<?xml version="1.0" encoding="utf-8" ?>
<C:calendar-multiget xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop>
    <C:calendar-data/>
  </D:prop>
  <D:href>/calendars/my-calendar/visible.ics</D:href>
  <D:href>/calendars/my-calendar/.hidden.ics</D:href>
</C:calendar-multiget>"#;

        let (status, body_str) = report_calendar_query(&server, report_body).await;
        assert_eq!(status, StatusCode::MULTI_STATUS);
        assert!(
            body_str.contains("Visible Event"),
            "visible event missing: {body_str}"
        );
        assert!(
            !body_str.contains("Hidden Event"),
            "hidden calendar-data must not be returned: {body_str}"
        );
        assert!(
            body_str.contains("/calendars/my-calendar/.hidden.ics")
                && body_str.contains("404 Not Found"),
            "hidden href must be 404: {body_str}"
        );
    }

    #[tokio::test]
    async fn test_report_on_hidden_calendar_is_404() {
        let server = setup_hidden_calendar_server().await;

        let report_body = r#"<?xml version="1.0" encoding="utf-8" ?>
<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop>
    <C:calendar-data/>
  </D:prop>
  <C:filter>
    <C:comp-filter name="VCALENDAR"/>
  </C:filter>
</C:calendar-query>"#;

        let req = Request::builder()
            .method("REPORT")
            .uri("/calendars/.hidden-cal/")
            .header("Depth", "1")
            .body(Body::from(report_body))
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn test_is_calendar_data() {
        let valid_ical = b"BEGIN:VCALENDAR\nVERSION:2.0\nEND:VCALENDAR\n";
        assert!(is_calendar_data(valid_ical));

        let invalid_data = b"This is not calendar data";
        assert!(!is_calendar_data(invalid_data));
    }

    #[test]
    fn test_extract_calendar_uid() {
        let ical_with_uid = "BEGIN:VCALENDAR\nUID:test-123@example.com\nEND:VCALENDAR";
        assert_eq!(
            extract_calendar_uid(ical_with_uid),
            Some("test-123@example.com".to_string())
        );

        let ical_without_uid = "BEGIN:VCALENDAR\nSUMMARY:Test\nEND:VCALENDAR";
        assert_eq!(extract_calendar_uid(ical_without_uid), None);
    }

    #[test]
    fn test_validate_calendar_data() {
        let valid_ical = r#"BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//Test//Test//EN
BEGIN:VEVENT
UID:test@example.com
DTSTART:20240101T120000Z
DTEND:20240101T130000Z
SUMMARY:Test
END:VEVENT
END:VCALENDAR"#;

        assert!(validate_calendar_data(valid_ical).is_ok());
    }

    #[tokio::test]
    async fn test_freebusy_query_not_implemented() {
        let server = setup_caldav_server2().await;

        let ics_data = create_ics_data("test-event-1", "Test Event");
        put_ics_data(&server, ics_data, "/calendars/my-calendar/event.ics").await;

        let transparent = r#"BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//Test//Test//EN
BEGIN:VEVENT
UID:transparent-1
DTSTART:20240101T140000Z
DTEND:20240101T150000Z
SUMMARY:Transparent
TRANSP:TRANSPARENT
END:VEVENT
END:VCALENDAR"#
            .to_string();
        put_ics_data(
            &server,
            transparent,
            "/calendars/my-calendar/transparent.ics",
        )
        .await;

        let report_body = r#"<?xml version="1.0" encoding="utf-8" ?>
<C:free-busy-query xmlns:C="urn:ietf:params:xml:ns:caldav">
  <C:time-range start="20240101T000000Z" end="20240201T000000Z"/>
</C:free-busy-query>"#;

        let req = Request::builder()
            .method("REPORT")
            .uri("/calendars/my-calendar")
            .body(Body::from(report_body.to_string()))
            .unwrap();

        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert!(
            content_type.contains("text/calendar"),
            "content-type: {content_type}"
        );
        let body_str = resp_to_string(resp).await;
        assert!(
            body_str.contains("BEGIN:VFREEBUSY"),
            "expected VFREEBUSY: {body_str}"
        );
        assert!(
            body_str.contains("FREEBUSY:20240101T120000Z/20240101T130000Z"),
            "expected busy period covering the opaque event: {body_str}"
        );
        assert!(
            !body_str.contains("20240101T140000Z"),
            "TRANSPARENT event must not appear: {body_str}"
        );

        let missing_range = r#"<?xml version="1.0" encoding="utf-8" ?>
<C:free-busy-query xmlns:C="urn:ietf:params:xml:ns:caldav">
</C:free-busy-query>"#;
        let req = Request::builder()
            .method("REPORT")
            .uri("/calendars/my-calendar")
            .body(Body::from(missing_range.to_string()))
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        // RFC 4791 9.9: start must precede end and both must be UTC DATE-TIME.
        for (start, end) in [
            ("20240201T000000Z", "20240101T000000Z"),
            ("20240101T000000", "20240201T000000Z"),
            ("20240101", "20240201"),
        ] {
            let bad_range = format!(
                r#"<?xml version="1.0" encoding="utf-8" ?>
<C:free-busy-query xmlns:C="urn:ietf:params:xml:ns:caldav">
  <C:time-range start="{start}" end="{end}"/>
</C:free-busy-query>"#
            );
            let req = Request::builder()
                .method("REPORT")
                .uri("/calendars/my-calendar")
                .body(Body::from(bad_range))
                .unwrap();
            let resp = server.handle(req).await;
            assert_eq!(
                resp.status(),
                StatusCode::BAD_REQUEST,
                "time-range {start}/{end} must be rejected"
            );
        }
    }

    #[tokio::test]
    async fn test_freebusy_query_on_plain_collection_is_forbidden() {
        let server = setup_caldav_server2().await;
        mkcol(&server, "/calendars/plain").await;

        let report_body = r#"<?xml version="1.0" encoding="utf-8" ?>
<C:free-busy-query xmlns:C="urn:ietf:params:xml:ns:caldav">
  <C:time-range start="20240101T000000Z" end="20240201T000000Z"/>
</C:free-busy-query>"#;

        // A plain MKCOL collection is not a calendar: RFC 4791 7.10 forbids the report.
        let req = Request::builder()
            .method("REPORT")
            .uri("/calendars/plain")
            .body(Body::from(report_body.to_string()))
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);

        // A missing collection is still a 404.
        let req = Request::builder()
            .method("REPORT")
            .uri("/calendars/missing")
            .body(Body::from(report_body.to_string()))
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        // calendar-query on a plain collection is rejected the same way.
        let query_body = r#"<?xml version="1.0" encoding="utf-8" ?>
<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop><C:calendar-data/></D:prop>
  <C:filter><C:comp-filter name="VCALENDAR"/></C:filter>
</C:calendar-query>"#;
        let req = Request::builder()
            .method("REPORT")
            .uri("/calendars/plain")
            .header("Depth", "1")
            .body(Body::from(query_body.to_string()))
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);

        // The MKCALENDAR calendar still answers.
        let req = Request::builder()
            .method("REPORT")
            .uri("/calendars/my-calendar")
            .body(Body::from(report_body.to_string()))
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body_str = resp_to_string(resp).await;
        assert!(
            body_str.contains("BEGIN:VFREEBUSY"),
            "expected VFREEBUSY: {body_str}"
        );
    }

    #[tokio::test]
    async fn test_calendar_put_match_header() {
        let server = setup_caldav_server2().await;
        let ics_data = create_ics_data("test-event-1", "Test Event");
        let uri = "/calendars/my-calendar/event.ics";
        put_ics_data(&server, ics_data, uri).await;

        // PROPFIND to get etag
        let propfind_body = r#"<?xml version="1.0" encoding="utf-8" ?>
        <D:propfind xmlns:D="DAV:">
        <D:prop>
            <D:getetag/>
        </D:prop>
        </D:propfind>"#;

        let req = Request::builder()
            .method("PROPFIND")
            .uri(uri)
            .header("Depth", "0")
            .body(Body::from(propfind_body))
            .unwrap();

        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::MULTI_STATUS);

        let body_str = resp_to_string(resp).await;
        assert!(body_str.contains("getetag"));
        // A full XML parse might be better, but for a test, we can check for the etag structure.
        // Assuming etag is returned within <D:getetag>...</D:getetag> tags.
        let etag_start = body_str.find("<D:getetag>").unwrap() + "<D:getetag>".len();
        let etag_end = body_str[etag_start..].find("</D:getetag>").unwrap() + etag_start;
        let etag = &body_str[etag_start..etag_end];
        println!("etag: {:#?}", etag);
        assert!(!etag.is_empty());

        // send again, but with MATCH header
        let ics_data = create_ics_data("test-event-1", "Test Event, rename1");
        let req = Request::builder()
            .method(Method::PUT)
            .uri(uri)
            .header("Content-Type", "text/calendar")
            .header("if-match", etag) // Use the obtained etag
            .body(Body::from(ics_data))
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::NO_CONTENT); // For an update, it should be NO_CONTENT
    }

    #[tokio::test]
    async fn test_current_user_principal_without_principal() {
        let server = setup_caldav_server().await;

        let propfind_body = r#"<?xml version="1.0" encoding="utf-8" ?>
<D:propfind xmlns:D="DAV:">
  <D:prop>
    <D:current-user-principal/>
  </D:prop>
</D:propfind>"#;

        let req = Request::builder()
            .method("PROPFIND")
            .uri("/")
            .header("Depth", "0")
            .body(Body::from(propfind_body))
            .unwrap();

        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::MULTI_STATUS);
        let body_str = resp_to_string(resp).await;
        assert!(
            body_str.contains("current-user-principal"),
            "missing current-user-principal: {body_str}"
        );
        assert!(
            body_str.contains("unauthenticated") || body_str.contains("href"),
            "expected unauthenticated or href, not a 404 miss: {body_str}"
        );
        assert!(
            body_str.contains("HTTP/1.1 200"),
            "current-user-principal should be 200: {body_str}"
        );
    }

    #[tokio::test]
    async fn test_calendars_specific_propfind_omits_home_set() {
        let server = setup_caldav_server().await;

        let propfind_body = r#"<?xml version="1.0" encoding="utf-8" ?>
<D:propfind xmlns:D="DAV:">
  <D:prop>
    <D:getcontentlength/>
  </D:prop>
</D:propfind>"#;

        let req = Request::builder()
            .method("PROPFIND")
            .uri("/calendars")
            .header("Depth", "0")
            .body(Body::from(propfind_body))
            .unwrap();

        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::MULTI_STATUS);
        let body_str = resp_to_string(resp).await;
        assert!(
            !body_str.contains("calendar-home-set"),
            "specific-prop PROPFIND must not inject calendar-home-set: {body_str}"
        );
    }

    #[tokio::test]
    async fn test_calendar_home_set_on_principal() {
        let server = DavHandler::builder()
            .filesystem(dav_server::memfs::MemFs::new())
            .locksystem(FakeLs::new())
            .principal("/principals/me")
            .build_handler();

        for uri in ["/principals", "/principals/me"] {
            let req = Request::builder()
                .method("MKCOL")
                .uri(uri)
                .body(Body::empty())
                .unwrap();
            let resp = server.handle(req).await;
            assert_eq!(resp.status(), StatusCode::CREATED);
        }

        let propfind_body = r#"<?xml version="1.0" encoding="utf-8" ?>
<D:propfind xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop>
    <C:calendar-home-set/>
  </D:prop>
</D:propfind>"#;

        let req = Request::builder()
            .method("PROPFIND")
            .uri("/principals/me")
            .header("Depth", "0")
            .body(Body::from(propfind_body))
            .unwrap();

        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::MULTI_STATUS);
        let body_str = resp_to_string(resp).await;
        assert!(
            body_str.contains("calendar-home-set"),
            "missing calendar-home-set: {body_str}"
        );
        assert!(
            body_str.contains("/calendars/"),
            "missing /calendars/ href: {body_str}"
        );
    }

    #[tokio::test]
    async fn test_calendar_home_set_on_root_without_principal() {
        let server = setup_caldav_server().await;

        let propfind_body = r#"<?xml version="1.0" encoding="utf-8" ?>
<D:propfind xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop>
    <C:calendar-home-set/>
  </D:prop>
</D:propfind>"#;

        let req = Request::builder()
            .method("PROPFIND")
            .uri("/")
            .header("Depth", "0")
            .body(Body::from(propfind_body))
            .unwrap();

        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::MULTI_STATUS);
        let body_str = resp_to_string(resp).await;
        assert!(
            body_str.contains("calendar-home-set"),
            "missing calendar-home-set: {body_str}"
        );
        assert!(
            body_str.contains("/calendars/"),
            "missing /calendars/ href: {body_str}"
        );
    }

    /// MemFs wrapper that keeps dead props but uses default `mark_calendar`
    /// (metadata `is_calendar` stays false).
    #[cfg(feature = "proppatch")]
    #[derive(Clone)]
    struct DeadPropCalFs(dav_server::memfs::MemFs);

    #[cfg(feature = "proppatch")]
    impl DeadPropCalFs {
        fn new() -> Box<Self> {
            Box::new(Self(*dav_server::memfs::MemFs::new()))
        }
    }

    #[cfg(feature = "proppatch")]
    impl dav_server::fs::DavFileSystem for DeadPropCalFs {
        fn open<'a>(
            &'a self,
            path: &'a dav_server::davpath::DavPath,
            options: dav_server::fs::OpenOptions,
        ) -> dav_server::fs::FsFuture<'a, Box<dyn dav_server::fs::DavFile>> {
            self.0.open(path, options)
        }

        fn read_dir<'a>(
            &'a self,
            path: &'a dav_server::davpath::DavPath,
            meta: dav_server::fs::ReadDirMeta,
        ) -> dav_server::fs::FsFuture<
            'a,
            dav_server::fs::FsStream<Box<dyn dav_server::fs::DavDirEntry>>,
        > {
            self.0.read_dir(path, meta)
        }

        fn metadata<'a>(
            &'a self,
            path: &'a dav_server::davpath::DavPath,
        ) -> dav_server::fs::FsFuture<'a, Box<dyn dav_server::fs::DavMetaData>> {
            self.0.metadata(path)
        }

        fn symlink_metadata<'a>(
            &'a self,
            path: &'a dav_server::davpath::DavPath,
        ) -> dav_server::fs::FsFuture<'a, Box<dyn dav_server::fs::DavMetaData>> {
            self.0.symlink_metadata(path)
        }

        fn create_dir<'a>(
            &'a self,
            path: &'a dav_server::davpath::DavPath,
        ) -> dav_server::fs::FsFuture<'a, ()> {
            self.0.create_dir(path)
        }

        fn have_props<'a>(
            &'a self,
            path: &'a dav_server::davpath::DavPath,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + 'a>> {
            self.0.have_props(path)
        }

        fn patch_props<'a>(
            &'a self,
            path: &'a dav_server::davpath::DavPath,
            patch: Vec<(bool, dav_server::fs::DavProp)>,
        ) -> dav_server::fs::FsFuture<'a, Vec<(StatusCode, dav_server::fs::DavProp)>> {
            self.0.patch_props(path, patch)
        }

        fn get_props<'a>(
            &'a self,
            path: &'a dav_server::davpath::DavPath,
            do_content: bool,
        ) -> dav_server::fs::FsFuture<'a, Vec<dav_server::fs::DavProp>> {
            self.0.get_props(path, do_content)
        }

        fn get_prop<'a>(
            &'a self,
            path: &'a dav_server::davpath::DavPath,
            prop: dav_server::fs::DavProp,
        ) -> dav_server::fs::FsFuture<'a, Vec<u8>> {
            self.0.get_prop(path, prop)
        }
    }

    #[cfg(feature = "proppatch")]
    #[tokio::test]
    async fn test_mkcalendar_dead_prop_marker_is_calendar() {
        let server = DavHandler::builder()
            .filesystem(DeadPropCalFs::new())
            .locksystem(FakeLs::new())
            .build_handler();
        mkcol(&server, "/calendars").await;

        let req = Request::builder()
            .method("MKCALENDAR")
            .uri("/calendars/prop-cal")
            .body(Body::empty())
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::CREATED);

        let propfind_body = r#"<?xml version="1.0" encoding="utf-8" ?>
<D:propfind xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop>
    <D:resourcetype/>
  </D:prop>
</D:propfind>"#;
        let req = Request::builder()
            .method("PROPFIND")
            .uri("/calendars/prop-cal")
            .header("Depth", "0")
            .body(Body::from(propfind_body))
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::MULTI_STATUS);
        let body_str = resp_to_string(resp).await;
        assert!(
            body_str.contains("<C:calendar"),
            "MKCALENDAR on a prop-only filesystem must still be a calendar: {body_str}"
        );
    }

    /// Largest chunk a `ShortReadFile` hands back per `read_bytes` call.
    const SHORT_READ_MAX: usize = 7;

    /// MemFs wrapper whose files return at most `SHORT_READ_MAX` bytes per
    /// `read_bytes` call, like a `Read::read` that short-reads.
    #[derive(Clone)]
    struct ShortReadFs(dav_server::memfs::MemFs);

    impl ShortReadFs {
        fn new() -> Box<Self> {
            Box::new(Self(*dav_server::memfs::MemFs::new()))
        }
    }

    #[derive(Debug)]
    struct ShortReadFile(Box<dyn dav_server::fs::DavFile>);

    impl dav_server::fs::DavFile for ShortReadFile {
        fn metadata(
            &'_ mut self,
        ) -> dav_server::fs::FsFuture<'_, Box<dyn dav_server::fs::DavMetaData>> {
            self.0.metadata()
        }

        fn write_buf(
            &'_ mut self,
            buf: Box<dyn bytes::Buf + Send>,
        ) -> dav_server::fs::FsFuture<'_, ()> {
            self.0.write_buf(buf)
        }

        fn write_bytes(&'_ mut self, buf: bytes::Bytes) -> dav_server::fs::FsFuture<'_, ()> {
            self.0.write_bytes(buf)
        }

        fn read_bytes(&'_ mut self, count: usize) -> dav_server::fs::FsFuture<'_, bytes::Bytes> {
            self.0.read_bytes(count.min(SHORT_READ_MAX))
        }

        fn seek(&'_ mut self, pos: std::io::SeekFrom) -> dav_server::fs::FsFuture<'_, u64> {
            self.0.seek(pos)
        }

        fn flush(&'_ mut self) -> dav_server::fs::FsFuture<'_, ()> {
            self.0.flush()
        }
    }

    impl dav_server::fs::DavFileSystem for ShortReadFs {
        fn open<'a>(
            &'a self,
            path: &'a dav_server::davpath::DavPath,
            options: dav_server::fs::OpenOptions,
        ) -> dav_server::fs::FsFuture<'a, Box<dyn dav_server::fs::DavFile>> {
            use futures_util::TryFutureExt;
            Box::pin(
                self.0.open(path, options).map_ok(|file| {
                    Box::new(ShortReadFile(file)) as Box<dyn dav_server::fs::DavFile>
                }),
            )
        }

        fn read_dir<'a>(
            &'a self,
            path: &'a dav_server::davpath::DavPath,
            meta: dav_server::fs::ReadDirMeta,
        ) -> dav_server::fs::FsFuture<
            'a,
            dav_server::fs::FsStream<Box<dyn dav_server::fs::DavDirEntry>>,
        > {
            self.0.read_dir(path, meta)
        }

        fn metadata<'a>(
            &'a self,
            path: &'a dav_server::davpath::DavPath,
        ) -> dav_server::fs::FsFuture<'a, Box<dyn dav_server::fs::DavMetaData>> {
            self.0.metadata(path)
        }

        fn symlink_metadata<'a>(
            &'a self,
            path: &'a dav_server::davpath::DavPath,
        ) -> dav_server::fs::FsFuture<'a, Box<dyn dav_server::fs::DavMetaData>> {
            self.0.symlink_metadata(path)
        }

        fn create_dir<'a>(
            &'a self,
            path: &'a dav_server::davpath::DavPath,
        ) -> dav_server::fs::FsFuture<'a, ()> {
            self.0.create_dir(path)
        }

        fn remove_file<'a>(
            &'a self,
            path: &'a dav_server::davpath::DavPath,
        ) -> dav_server::fs::FsFuture<'a, ()> {
            self.0.remove_file(path)
        }

        fn mark_calendar<'a>(
            &'a self,
            path: &'a dav_server::davpath::DavPath,
        ) -> dav_server::fs::FsFuture<'a, ()> {
            self.0.mark_calendar(path)
        }
    }

    #[tokio::test]
    async fn test_calendar_reports_tolerate_short_reads() {
        let server = DavHandler::builder()
            .filesystem(ShortReadFs::new())
            .locksystem(FakeLs::new())
            .build_handler();
        mkcol(&server, "/calendars").await;

        let req = Request::builder()
            .method("MKCALENDAR")
            .uri("/calendars/short-cal")
            .body(Body::empty())
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::CREATED);

        let ics_data = create_ics_data("short-read-event", "Short Read Event");
        assert!(ics_data.len() > SHORT_READ_MAX);
        let resp = put_ics_data(&server, ics_data, "/calendars/short-cal/event.ics").await;
        assert_eq!(resp.status(), StatusCode::CREATED);

        let report_body = r#"<?xml version="1.0" encoding="utf-8" ?>
<C:calendar-multiget xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop>
    <C:calendar-data/>
  </D:prop>
  <D:href>/calendars/short-cal/event.ics</D:href>
</C:calendar-multiget>"#;
        let req = Request::builder()
            .method("REPORT")
            .uri("/calendars/short-cal")
            .body(Body::from(report_body))
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::MULTI_STATUS);
        let body_str = resp_to_string(resp).await;
        assert!(
            !body_str.contains("404"),
            "multiget must not 404 a resource on a short-reading filesystem: {body_str}"
        );
        assert!(
            body_str.contains("Short Read Event") && body_str.contains("END:VCALENDAR"),
            "multiget must return the whole object: {body_str}"
        );

        let report_body = r#"<?xml version="1.0" encoding="utf-8" ?>
<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop>
    <C:calendar-data/>
  </D:prop>
  <C:filter>
    <C:comp-filter name="VCALENDAR">
      <C:comp-filter name="VEVENT"/>
    </C:comp-filter>
  </C:filter>
</C:calendar-query>"#;
        let req = Request::builder()
            .method("REPORT")
            .uri("/calendars/short-cal")
            .header("Depth", "1")
            .body(Body::from(report_body))
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::MULTI_STATUS);
        let body_str = resp_to_string(resp).await;
        assert!(
            body_str.contains("Short Read Event") && body_str.contains("END:VCALENDAR"),
            "calendar-query must return the whole object: {body_str}"
        );
    }

    #[cfg(feature = "proppatch")]
    async fn mkcalendar_on_dead_prop_fs() -> DavHandler {
        let server = DavHandler::builder()
            .filesystem(DeadPropCalFs::new())
            .locksystem(FakeLs::new())
            .build_handler();
        mkcol(&server, "/calendars").await;
        let req = Request::builder()
            .method("MKCALENDAR")
            .uri("/calendars/prop-cal")
            .body(Body::empty())
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::CREATED);
        server
    }

    #[cfg(feature = "proppatch")]
    async fn proppatch(server: &DavHandler, uri: &str, body: &str) -> String {
        let req = Request::builder()
            .method("PROPPATCH")
            .uri(uri)
            .body(Body::from(body.to_string()))
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::MULTI_STATUS, "PROPPATCH {uri}");
        resp_to_string(resp).await
    }

    #[cfg(feature = "proppatch")]
    async fn propfind_resourcetype(server: &DavHandler, uri: &str) -> String {
        let propfind_body = r#"<?xml version="1.0" encoding="utf-8" ?>
<D:propfind xmlns:D="DAV:">
  <D:prop>
    <D:resourcetype/>
  </D:prop>
</D:propfind>"#;
        let req = Request::builder()
            .method("PROPFIND")
            .uri(uri)
            .header("Depth", "0")
            .body(Body::from(propfind_body))
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::MULTI_STATUS, "PROPFIND {uri}");
        resp_to_string(resp).await
    }

    #[cfg(feature = "proppatch")]
    const PROPPATCH_SET_MARKER: &str = r#"<?xml version="1.0" encoding="utf-8" ?>
<D:propertyupdate xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:set>
    <D:prop>
      <C:calendar/>
    </D:prop>
  </D:set>
</D:propertyupdate>"#;

    #[cfg(feature = "proppatch")]
    const PROPPATCH_REMOVE_MARKER: &str = r#"<?xml version="1.0" encoding="utf-8" ?>
<D:propertyupdate xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:remove>
    <D:prop>
      <C:calendar/>
    </D:prop>
  </D:remove>
</D:propertyupdate>"#;

    /// PROPPATCH must not be able to plant the calendar type marker on a
    /// plain collection and thereby turn it into a calendar.
    #[cfg(feature = "proppatch")]
    #[tokio::test]
    async fn test_proppatch_cannot_set_calendar_type_marker() {
        let server = setup_caldav_server().await;
        mkcol(&server, "/calendars/plain").await;

        let body_str = proppatch(&server, "/calendars/plain", PROPPATCH_SET_MARKER).await;
        assert!(
            body_str.contains("HTTP/1.1 403"),
            "setting the type marker must be forbidden: {body_str}"
        );

        let body_str = propfind_resourcetype(&server, "/calendars/plain").await;
        assert!(
            body_str.contains("<D:collection"),
            "missing D:collection: {body_str}"
        );
        assert!(
            !body_str.contains("<C:calendar"),
            "plain collection must not have become a calendar: {body_str}"
        );
    }

    /// PROPPATCH must not be able to remove the marker that makes a
    /// collection a calendar on a filesystem that stores it as a dead prop.
    #[cfg(feature = "proppatch")]
    #[tokio::test]
    async fn test_proppatch_cannot_remove_calendar_type_marker() {
        let server = mkcalendar_on_dead_prop_fs().await;

        let body_str = proppatch(&server, "/calendars/prop-cal", PROPPATCH_REMOVE_MARKER).await;
        assert!(
            body_str.contains("HTTP/1.1 403"),
            "removing the type marker must be forbidden: {body_str}"
        );

        let body_str = propfind_resourcetype(&server, "/calendars/prop-cal").await;
        assert!(
            body_str.contains("<C:calendar"),
            "calendar must still be a calendar: {body_str}"
        );
    }

    /// The stored type marker is an implementation detail: PROPFIND allprop
    /// must only report it inside DAV:resourcetype, not as a dead property.
    #[cfg(feature = "proppatch")]
    #[tokio::test]
    async fn test_propfind_allprop_hides_calendar_type_marker() {
        let server = mkcalendar_on_dead_prop_fs().await;

        let req = Request::builder()
            .method("PROPFIND")
            .uri("/calendars/prop-cal")
            .header("Depth", "0")
            .body(Body::empty())
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::MULTI_STATUS);
        let body_str = resp_to_string(resp).await;

        let resourcetype_end = body_str
            .find("</D:resourcetype>")
            .unwrap_or_else(|| panic!("missing resourcetype: {body_str}"));
        assert!(
            body_str[..resourcetype_end].contains("<C:calendar"),
            "resourcetype must contain C:calendar: {body_str}"
        );
        assert_eq!(
            body_str.matches("<C:calendar").count(),
            1,
            "type marker leaked as a dead property: {body_str}"
        );
    }
}

#[cfg(all(not(feature = "caldav"), feature = "memfs"))]
mod caldav_disabled_tests {
    use dav_server::{DavHandler, body::Body, fakels::FakeLs, memfs::MemFs};
    use http::Request;

    #[tokio::test]
    async fn test_caldav_methods_return_not_implemented() {
        let server = DavHandler::builder()
            .filesystem(MemFs::new())
            .locksystem(FakeLs::new())
            .build_handler();

        // Test REPORT method - only returns NOT_IMPLEMENTED when carddav is also disabled
        #[cfg(not(feature = "carddav"))]
        {
            let req = Request::builder()
                .method("REPORT")
                .uri("/")
                .body(Body::empty())
                .unwrap();
            let resp = server.handle(req).await;
            assert_eq!(resp.status(), http::StatusCode::NOT_IMPLEMENTED);
        }

        // Test MKCALENDAR method
        let req = Request::builder()
            .method("MKCALENDAR")
            .uri("/calendars/my-calendar")
            .body(Body::empty())
            .unwrap();
        let resp = server.handle(req).await;
        assert_eq!(resp.status(), http::StatusCode::NOT_IMPLEMENTED);
    }
}
