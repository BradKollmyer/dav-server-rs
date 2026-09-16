#![cfg(all(feature = "localfs", any(feature = "caldav", feature = "carddav")))]
use dav_server::{DavHandler, DavOptionHide, body::Body, localfs::LocalFs};
use http::{Request, StatusCode};
use http_body_util::BodyExt;

struct Fixture(std::path::PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
async fn req(
    server: &DavHandler,
    method: &str,
    path: &str,
    destination: Option<&str>,
) -> (StatusCode, String) {
    let mut request = Request::builder().method(method).uri(path);
    if method == "PROPFIND" {
        request = request.header("Depth", "1");
    }
    if let Some(destination) = destination {
        request = request.header("Destination", destination);
    }
    let body = if method == "REPORT" {
        Body::from("<report/>")
    } else {
        Body::empty()
    };
    let response = server.handle(request.body(body).unwrap()).await;
    let status = response.status();
    let text = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8(text.to_vec()).unwrap())
}

async fn check_marker(creation: &str, marker: &str) {
    let fixture =
        Fixture(std::env::temp_dir().join(format!("dav-markers-{}", uuid::Uuid::new_v4())));
    std::fs::create_dir(&fixture.0).unwrap();
    let server = DavHandler::builder()
        .filesystem(LocalFs::new(&fixture.0, false, true, false))
        .hide_dot_prefix(DavOptionHide::Never)
        .autoindex(true)
        .build_handler();
    assert_eq!(
        req(&server, creation, "/collection", None).await.0,
        StatusCode::CREATED
    );
    assert!(fixture.0.join("collection").join(marker).exists());
    std::fs::write(fixture.0.join("ordinary"), b"data").unwrap();
    for name in [marker.to_owned(), marker.to_ascii_uppercase()] {
        let target = format!("/collection/{name}");
        for method in [
            "GET",
            "PUT",
            "DELETE",
            "MKCOL",
            "PROPFIND",
            "PROPPATCH",
            "REPORT",
        ] {
            assert_eq!(
                req(&server, method, &target, None).await.0,
                StatusCode::NOT_FOUND,
                "{method} {target}"
            );
        }
        for method in ["COPY", "MOVE"] {
            assert_eq!(
                req(&server, method, &target, Some("/exported")).await.0,
                StatusCode::NOT_FOUND
            );
            assert_eq!(
                req(&server, method, "/ordinary", Some(&target)).await.0,
                StatusCode::NOT_FOUND
            );
        }
        assert_eq!(
            req(&server, "PUT", &format!("/{name}"), None).await.0,
            StatusCode::NOT_FOUND
        );
    }
    for method in ["GET", "PROPFIND"] {
        let (_, listing) = req(&server, method, "/collection/", None).await;
        assert!(!listing.contains(marker), "{listing}");
    }
    assert!(fixture.0.join("collection").join(marker).exists());
    assert_eq!(
        req(&server, "MOVE", "/collection", Some("/renamed"))
            .await
            .0,
        StatusCode::CREATED
    );
    assert!(fixture.0.join("renamed").join(marker).exists());
    assert_eq!(
        req(&server, "DELETE", "/renamed", None).await.0,
        StatusCode::NO_CONTENT
    );
    assert!(!fixture.0.join("renamed").exists());
}
#[cfg(feature = "caldav")]
#[tokio::test]
async fn calendar_marker_is_private_but_collection_operations_work() {
    check_marker("MKCALENDAR", ".dav-calendar").await;
}
#[cfg(feature = "carddav")]
#[tokio::test]
async fn addressbook_marker_is_private_but_collection_operations_work() {
    check_marker("MKADDRESSBOOK", ".dav-addressbook").await;
}
