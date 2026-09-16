#![cfg(all(feature = "localfs", feature = "memfs", unix))]
use dav_server::{DavHandler, DavOptionHide, body::Body};
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

fn local(case_insensitive: bool) -> (DavHandler, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!("dav-review-copy-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    let s = DavHandler::builder()
        .filesystem(dav_server::localfs::LocalFs::new(
            &root,
            false,
            case_insensitive,
            false,
        ))
        .hide_dot_prefix(DavOptionHide::Always)
        .hide_symlinks(true)
        .build_handler();
    (s, root)
}

#[tokio::test]
async fn aliases_cannot_destroy_copy_or_move_source() {
    for method in ["COPY", "MOVE"] {
        let (s, root) = local(true);
        std::fs::write(root.join("original"), "unique data").unwrap();
        assert_eq!(
            req(&s, method, "/original", "", &[("Destination", "/ORIGINAL")])
                .await
                .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            std::fs::read(root.join("original")).unwrap(),
            b"unique data"
        );
        std::fs::hard_link(root.join("original"), root.join("alias")).unwrap();
        assert_eq!(
            req(&s, method, "/original", "", &[("Destination", "/alias")])
                .await
                .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            std::fs::read(root.join("original")).unwrap(),
            b"unique data"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
