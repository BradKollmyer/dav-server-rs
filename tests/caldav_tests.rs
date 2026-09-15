#[cfg(all(feature = "caldav", feature = "memfs"))]
mod caldav_tests {
    use dav_server::{DavHandler, body::Body, caldav::*, fakels::FakeLs};
    use http::response::Response;
    use http::{Method, Request, StatusCode};

    fn setup_caldav_server() -> DavHandler {
        DavHandler::builder()
            .filesystem(dav_server::memfs::MemFs::new())
            // .filesystem(dav_server::localfs::LocalFs::new("/tmp", true, false, false))
            .locksystem(FakeLs::new())
            .build_handler()
    }

    async fn setup_caldav_server2() -> DavHandler {
        let server = setup_caldav_server();

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
        let server = setup_caldav_server();

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
        let server = setup_caldav_server();

        let req = Request::builder()
            .method("MKCALENDAR")
            .uri("/calendars/my-calendar")
            .body(Body::empty())
            .unwrap();

        let resp = server.handle(req).await;
        assert_eq!(resp.status(), StatusCode::CREATED);
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
        let server = setup_caldav_server();

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
        let server = setup_caldav_server();

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
        let server = setup_caldav_server();

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
        assert!(!body_str.contains("addressbook-query"));
        assert!(!body_str.contains("free-busy-query"));
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

    #[tokio::test]
    async fn test_calendar_query_report_hrefs_include_strip_prefix() {
        let server = DavHandler::builder()
            .filesystem(dav_server::memfs::MemFs::new())
            .locksystem(FakeLs::new())
            .strip_prefix("/dav")
            .build_handler();

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
        assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
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
        let server = setup_caldav_server();

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
        let server = setup_caldav_server();

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
        let server = setup_caldav_server();

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
