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

#[cfg(feature = "carddav")]
#[tokio::test]
async fn addressbook_query_combines_every_property_filter() {
    let s = DavHandler::builder()
        .filesystem(dav_server::memfs::MemFs::new())
        .build_handler();
    assert_eq!(
        req(&s, "MKADDRESSBOOK", "/book", "", &[]).await.0,
        StatusCode::CREATED
    );
    let card = "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:one\r\nFN:Alice\r\nEMAIL:alice@example.com\r\nTEL;TYPE=HOME:555\r\nEND:VCARD\r\n";
    assert_eq!(
        req(&s, "PUT", "/book/a.vcf", card, &[]).await.0,
        StatusCode::CREATED
    );
    for (test, name, email, matched) in [
        ("allof", "Bob", "example.com", false),
        ("allof", "Alice", "elsewhere.com", false),
        ("allof", "Alice", "example.com", true),
        ("anyof", "Alice", "elsewhere.com", true),
        ("anyof", "Bob", "example.com", true),
        ("anyof", "Bob", "elsewhere.com", false),
        ("", "Alice", "elsewhere.com", true),
        ("", "Bob", "example.com", true),
    ] {
        let attr = if test.is_empty() {
            String::new()
        } else {
            format!(" test=\"{test}\"")
        };
        let query = format!(
            r#"<A:addressbook-query xmlns:A="urn:ietf:params:xml:ns:carddav"><A:filter{attr}><A:prop-filter name="FN"><A:text-match>{name}</A:text-match></A:prop-filter><A:prop-filter name="EMAIL"><A:text-match>{email}</A:text-match></A:prop-filter></A:filter></A:addressbook-query>"#
        );
        let result = req(&s, "REPORT", "/book", &query, &[("Depth", "1")]).await;
        assert_eq!(result.0, StatusCode::MULTI_STATUS);
        assert_eq!(
            result.1.contains("/book/a.vcf"),
            matched,
            "test={test} name={name} email={email}: {}",
            result.1
        );
    }
    for (filter, matched) in [
        (
            r#"<A:filter test="allof"><A:prop-filter name="FN"/><A:prop-filter name="NICKNAME"><A:is-not-defined/></A:prop-filter></A:filter>"#,
            true,
        ),
        (
            r#"<A:filter test="allof"><A:prop-filter name="FN"/><A:prop-filter name="EMAIL"><A:is-not-defined/></A:prop-filter></A:filter>"#,
            false,
        ),
        (r#"<A:filter/>"#, true),
        (
            r#"<A:filter><A:prop-filter name="FN"><A:text-match>Alice</A:text-match><A:text-match>Bob</A:text-match></A:prop-filter></A:filter>"#,
            true,
        ),
        (
            r#"<A:filter><A:prop-filter name="FN" test="allof"><A:text-match>Alice</A:text-match><A:text-match>Bob</A:text-match></A:prop-filter></A:filter>"#,
            false,
        ),
        (
            r#"<A:filter><A:prop-filter name="TEL"><A:text-match>999</A:text-match><A:param-filter name="TYPE"><A:text-match>HOME</A:text-match></A:param-filter></A:prop-filter></A:filter>"#,
            true,
        ),
        (
            r#"<A:filter><A:prop-filter name="TEL" test="allof"><A:text-match>999</A:text-match><A:param-filter name="TYPE"><A:text-match>HOME</A:text-match></A:param-filter></A:prop-filter></A:filter>"#,
            false,
        ),
    ] {
        let query = format!(
            r#"<A:addressbook-query xmlns:A="urn:ietf:params:xml:ns:carddav">{filter}</A:addressbook-query>"#
        );
        let result = req(&s, "REPORT", "/book", &query, &[("Depth", "1")]).await;
        assert_eq!(result.0, StatusCode::MULTI_STATUS);
        assert_eq!(
            result.1.contains("/book/a.vcf"),
            matched,
            "{filter}: {}",
            result.1
        );
    }
    for filter in [
        r#"<A:filter test="bogus"/>"#,
        r#"<A:filter><A:prop-filter name="FN" test="bogus"/></A:filter>"#,
    ] {
        let query = format!(
            r#"<A:addressbook-query xmlns:A="urn:ietf:params:xml:ns:carddav">{filter}</A:addressbook-query>"#
        );
        assert_eq!(
            req(&s, "REPORT", "/book", &query, &[("Depth", "1")])
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
}
