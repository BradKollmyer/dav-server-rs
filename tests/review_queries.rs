#![cfg(feature = "memfs")]
use dav_server::{DavHandler, body::Body};
use http::{Request, StatusCode};
use http_body_util::BodyExt;

async fn req(
    s: &DavHandler,
    method: &str,
    path: &str,
    body: &str,
    headers: &[(&str, &str)],
) -> (StatusCode, String) {
    let mut r = Request::builder().method(method).uri(path);
    for (k, v) in headers {
        r = r.header(*k, *v);
    }
    let r = s.handle(r.body(Body::from(body.to_owned())).unwrap()).await;
    let status = r.status();
    let text =
        String::from_utf8(r.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
    (status, text)
}

#[cfg(feature = "caldav")]
#[tokio::test]
async fn calendar_query_evaluates_objects_and_respects_zero_depth() {
    let s = DavHandler::builder()
        .filesystem(dav_server::memfs::MemFs::new())
        .build_handler();
    assert_eq!(
        req(&s, "MKCALENDAR", "/cal", "", &[]).await.0,
        StatusCode::CREATED
    );
    let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Test//EN\r\nBEGIN:VEVENT\r\nUID:one\r\nDTSTART:20260101T120000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    for path in ["/cal/a.ics", "/cal/b.ics"] {
        assert_eq!(req(&s, "PUT", path, ics, &[]).await.0, StatusCode::CREATED);
    }
    let query = r#"<C:calendar-query xmlns:C="urn:ietf:params:xml:ns:caldav"><C:filter><C:comp-filter name="VCALENDAR"><C:comp-filter name="VEVENT"/></C:comp-filter></C:filter></C:calendar-query>"#;
    for headers in [vec![], vec![("Depth", "0")], vec![("Depth", "1")]] {
        let result = req(&s, "REPORT", "/cal/a.ics", query, &headers).await;
        assert_eq!(result.0, StatusCode::MULTI_STATUS, "{}", result.1);
        assert!(result.1.contains("/cal/a.ics"));
        assert!(!result.1.contains("/cal/b.ics"));
    }
    let unmatched = query.replace("VEVENT", "VTODO");
    let result = req(&s, "REPORT", "/cal/a.ics", &unmatched, &[("Depth", "0")]).await;
    assert_eq!(result.0, StatusCode::MULTI_STATUS);
    assert!(!result.1.contains("/cal/a.ics"));
    for headers in [vec![], vec![("Depth", "0")]] {
        let result = req(&s, "REPORT", "/cal", query, &headers).await;
        assert_eq!(result.0, StatusCode::MULTI_STATUS);
        assert!(!result.1.contains("/cal/a.ics"));
        assert!(!result.1.contains("/cal/b.ics"));
    }
    let result = req(&s, "REPORT", "/cal", query, &[("Depth", "1")]).await;
    assert_eq!(result.0, StatusCode::MULTI_STATUS);
    assert!(result.1.contains("/cal/a.ics") && result.1.contains("/cal/b.ics"));
    assert_eq!(
        req(&s, "REPORT", "/cal", query, &[("Depth", "invalid")])
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
}
