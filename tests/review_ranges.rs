#![cfg(feature = "memfs")]
use dav_server::{DavHandler, body::Body, memfs::MemFs};
use http::{Request, StatusCode};
use http_body_util::BodyExt;

#[tokio::test]
async fn overflowing_patch_range_is_rejected_without_changing_existing_file() {
    let server = DavHandler::builder()
        .filesystem(MemFs::new())
        .build_handler();
    let response = server
        .handle(
            Request::builder()
                .method("PUT")
                .uri("/file")
                .body(Body::from("original"))
                .unwrap(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    for (method, name, value) in [
        ("PATCH", "X-Update-Range", "bytes=0-18446744073709551615"),
        ("PUT", "Content-Range", "bytes 0-18446744073709551615/*"),
    ] {
        let response = server
            .handle(
                Request::builder()
                    .method(method)
                    .uri("/file")
                    .header("Content-Type", "application/x-sabredav-partialupdate")
                    .header("Content-Length", "0")
                    .header(name, value)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        // headers may reject Content-Range before the handler can inspect it.
        assert!(
            matches!(
                response.status(),
                StatusCode::RANGE_NOT_SATISFIABLE | StatusCode::BAD_REQUEST
            ),
            "{method}: {}",
            response.status()
        );
        let response = server
            .handle(Request::builder().uri("/file").body(Body::empty()).unwrap())
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.into_body().collect().await.unwrap().to_bytes(),
            "original"
        );
    }
}
