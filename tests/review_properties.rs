#![cfg(all(feature = "caldav", feature = "carddav", feature = "memfs"))]

use dav_server::{DavHandler, body::Body};
use http::{Request, StatusCode};
use http_body_util::BodyExt;

async fn request(
    server: &DavHandler,
    method: &str,
    path: &str,
    body: &str,
) -> (StatusCode, String) {
    let response = server
        .handle(
            Request::builder()
                .method(method)
                .uri(path)
                .header("Depth", if method == "REPORT" { "1" } else { "0" })
                .body(Body::from(body.to_owned()))
                .unwrap(),
        )
        .await;
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8(body.to_vec()).unwrap())
}

#[tokio::test]
async fn typed_resourcetype_has_bound_namespace_at_arbitrary_paths() {
    let server = DavHandler::builder()
        .filesystem(dav_server::memfs::MemFs::new())
        .build_handler();
    for (method, path, name, namespace) in [
        (
            "MKCALENDAR",
            "/cal",
            "calendar",
            "urn:ietf:params:xml:ns:caldav",
        ),
        (
            "MKADDRESSBOOK",
            "/people",
            "addressbook",
            "urn:ietf:params:xml:ns:carddav",
        ),
    ] {
        assert_eq!(
            request(&server, method, path, "").await.0,
            StatusCode::CREATED
        );
        for body in [
            "",
            r#"<D:propfind xmlns:D="DAV:"><D:prop><D:resourcetype/></D:prop></D:propfind>"#,
        ] {
            let (status, response) = request(&server, "PROPFIND", path, body).await;
            assert_eq!(status, StatusCode::MULTI_STATUS);
            let tree = xmltree::Element::parse(response.as_bytes()).unwrap();
            let prop = tree
                .get_child("response")
                .unwrap()
                .get_child("propstat")
                .unwrap()
                .get_child("prop")
                .unwrap();
            let typed = prop
                .get_child("resourcetype")
                .unwrap()
                .get_child(name)
                .unwrap();
            assert_eq!(typed.namespace.as_deref(), Some(namespace));
        }
    }
}

const ICS: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Test//EN\r\nBEGIN:VEVENT\r\nUID:one\r\nDTSTART:20260101T120000Z\r\nDTEND:20260101T130000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
const VCARD: &str =
    "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:one\r\nFN:Alice\r\nN:Alice;;;;\r\nEND:VCARD\r\n";

#[tokio::test]
async fn property_and_report_etags_match_get_header() {
    let server = DavHandler::builder()
        .filesystem(dav_server::memfs::MemFs::new())
        .build_handler();
    for (method, path, file, data, namespace, report) in [
        (
            "MKCALENDAR",
            "/cal",
            "/cal/a.ics",
            ICS,
            "urn:ietf:params:xml:ns:caldav",
            "calendar",
        ),
        (
            "MKADDRESSBOOK",
            "/people",
            "/people/a.vcf",
            VCARD,
            "urn:ietf:params:xml:ns:carddav",
            "addressbook",
        ),
    ] {
        assert_eq!(
            request(&server, method, path, "").await.0,
            StatusCode::CREATED
        );
        assert_eq!(
            request(&server, "PUT", file, data).await.0,
            StatusCode::CREATED
        );
        let get = server
            .handle(Request::builder().uri(file).body(Body::empty()).unwrap())
            .await;
        let expected = get
            .headers()
            .get("ETag")
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        assert!(expected.starts_with('"') && expected.ends_with('"'));
        let filter = if report == "calendar" {
            r#"<T:filter><T:comp-filter name="VCALENDAR"/></T:filter>"#
        } else {
            ""
        };
        let query = format!(
            r#"<T:{report}-query xmlns:T="{namespace}" xmlns:D="DAV:"><D:prop><D:getetag/></D:prop>{filter}</T:{report}-query>"#
        );
        let multiget = format!(
            r#"<T:{report}-multiget xmlns:T="{namespace}" xmlns:D="DAV:"><D:prop><D:getetag/></D:prop><D:href>{file}</D:href></T:{report}-multiget>"#
        );
        for (verb, target, body) in [
            (
                "PROPFIND",
                file,
                r#"<D:propfind xmlns:D="DAV:"><D:prop><D:getetag/></D:prop></D:propfind>"#,
            ),
            ("REPORT", path, query.as_str()),
            ("REPORT", path, multiget.as_str()),
        ] {
            let (status, xml) = request(&server, verb, target, body).await;
            assert_eq!(status, StatusCode::MULTI_STATUS, "{verb} {target}: {xml}");
            let tree = xmltree::Element::parse(xml.as_bytes()).unwrap();
            let etag = tree
                .get_child("response")
                .unwrap()
                .get_child("propstat")
                .unwrap()
                .get_child("prop")
                .unwrap()
                .get_child("getetag")
                .unwrap()
                .get_text()
                .unwrap();
            assert_eq!(etag, expected, "{verb} {target}");
        }
    }
}
