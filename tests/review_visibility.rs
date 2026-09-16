#![cfg(all(unix, feature = "localfs"))]

use dav_server::{DavHandler, DavOptionHide, body::Body, localfs::LocalFs};
use http::{Request, StatusCode};
use http_body_util::BodyExt;

struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("dav-visibility-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join(".secret")).unwrap();
        std::fs::create_dir_all(root.join("real")).unwrap();
        std::fs::write(root.join(".secret/key"), b"hidden").unwrap();
        std::fs::write(root.join("real/key"), b"original").unwrap();
        std::os::unix::fs::symlink("real", root.join("link")).unwrap();
        Self(root)
    }
    fn server(&self, hide: bool) -> DavHandler {
        DavHandler::builder()
            .filesystem(LocalFs::new(&self.0, false, false, false))
            .strip_prefix("/mount")
            .hide_dot_prefix(if hide {
                DavOptionHide::Always
            } else {
                DavOptionHide::Never
            })
            .hide_symlinks(hide)
            .build_handler()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn request(
    server: &DavHandler,
    method: &str,
    path: &str,
    destination: Option<&str>,
) -> StatusCode {
    let mut req = Request::builder().method(method).uri(path);
    if method == "PROPFIND" {
        req = req.header("Depth", "0");
    }
    if let Some(destination) = destination {
        req = req.header("Destination", destination);
    }
    let response = server.handle(req.body(Body::empty()).unwrap()).await;
    let status = response.status();
    response.into_body().collect().await.unwrap();
    status
}

#[tokio::test]
async fn hidden_ancestors_block_reads_and_mutations_under_mount_prefix() {
    let fixture = Fixture::new();
    let server = fixture.server(true);
    for ancestor in [".secret", "link"] {
        let path = format!("/mount/{ancestor}/key");
        for method in ["GET", "PUT", "DELETE", "MKCOL", "PROPFIND"] {
            assert_eq!(
                request(&server, method, &path, None).await,
                StatusCode::NOT_FOUND,
                "{method} {path}"
            );
        }
        for method in ["COPY", "MOVE"] {
            assert_eq!(
                request(&server, method, &path, Some("/mount/copied")).await,
                StatusCode::NOT_FOUND
            );
            assert_eq!(
                request(&server, method, "/mount/real/key", Some(&path)).await,
                StatusCode::NOT_FOUND
            );
        }
        assert_eq!(
            request(&server, "PUT", &format!("/mount/{ancestor}/new"), None).await,
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        std::fs::read(fixture.0.join("real/key")).unwrap(),
        b"original"
    );
    assert_eq!(
        std::fs::read(fixture.0.join(".secret/key")).unwrap(),
        b"hidden"
    );
    assert_eq!(
        request(&server, "GET", "/mount/real/key", None).await,
        StatusCode::OK
    );
    assert_eq!(
        request(&server, "PUT", "/mount/real/new", None).await,
        StatusCode::CREATED
    );
}

#[tokio::test]
async fn visible_policy_still_allows_hidden_names_and_in_share_symlinks() {
    let fixture = Fixture::new();
    let server = fixture.server(false);
    for path in ["/mount/.secret/key", "/mount/link/key"] {
        assert_eq!(request(&server, "GET", path, None).await, StatusCode::OK);
    }
}
