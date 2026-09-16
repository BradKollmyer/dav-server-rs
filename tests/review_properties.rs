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
                .header("Depth", "0")
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
