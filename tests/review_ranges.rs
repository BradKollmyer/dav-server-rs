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

async fn range_write(
    server: &DavHandler,
    method: &str,
    uri: &str,
    name: &str,
    value: &str,
    body: &str,
) -> http::Response<Body> {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("Content-Length", body.len().to_string())
        .header(name, value);
    if method == "PATCH" {
        req = req.header("Content-Type", "application/x-sabredav-partialupdate");
    }
    server.handle(req.body(Body::from(body)).unwrap()).await
}

async fn get_body(server: &DavHandler, uri: &str) -> (StatusCode, bytes::Bytes) {
    let response = server
        .handle(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await;
    (
        response.status(),
        response.into_body().collect().await.unwrap().to_bytes(),
    )
}

#[tokio::test]
async fn range_write_past_end_of_existing_file_is_rejected() {
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
    // "original" is 8 bytes; start 10 is past EOF.
    for (method, name, value) in [
        ("PUT", "Content-Range", "bytes 10-13/*"),
        ("PATCH", "X-Update-Range", "bytes=10-13"),
        ("PATCH", "X-Update-Range", "bytes=10-"),
    ] {
        let response = range_write(&server, method, "/file", name, value, "XXXX").await;
        assert_eq!(
            response.status(),
            StatusCode::RANGE_NOT_SATISFIABLE,
            "{method} {value}"
        );
        let (status, body) = get_body(&server, "/file").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "original");
    }
}

#[tokio::test]
async fn range_write_at_end_of_existing_file_appends() {
    let server = DavHandler::builder()
        .filesystem(MemFs::new())
        .build_handler();
    // start == len (8) is a contiguous append, not a sparse hole.
    for (i, (method, name, value)) in [
        ("PUT", "Content-Range", "bytes 8-11/*"),
        ("PATCH", "X-Update-Range", "bytes=8-11"),
        ("PATCH", "X-Update-Range", "bytes=8-"),
    ]
    .into_iter()
    .enumerate()
    {
        let uri = format!("/file{i}");
        let response = server
            .handle(
                Request::builder()
                    .method("PUT")
                    .uri(&uri)
                    .body(Body::from("original"))
                    .unwrap(),
            )
            .await;
        assert_eq!(response.status(), StatusCode::CREATED);
        let response = range_write(&server, method, &uri, name, value, "XXXX").await;
        assert_eq!(
            response.status(),
            StatusCode::NO_CONTENT,
            "{method} {value}"
        );
        let (status, body) = get_body(&server, &uri).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "originalXXXX");
    }
}

#[tokio::test]
async fn range_write_on_missing_file_may_start_past_zero() {
    let server = DavHandler::builder()
        .filesystem(MemFs::new())
        .build_handler();
    for (i, (method, name, value)) in [
        ("PUT", "Content-Range", "bytes 10-13/*"),
        ("PATCH", "X-Update-Range", "bytes=10-13"),
        ("PATCH", "X-Update-Range", "bytes=10-"),
    ]
    .into_iter()
    .enumerate()
    {
        let uri = format!("/new{i}");
        let (status, _) = get_body(&server, &uri).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let response = range_write(&server, method, &uri, name, value, "XXXX").await;
        assert_eq!(response.status(), StatusCode::CREATED, "{method} {value}");
        let (status, body) = get_body(&server, &uri).await;
        assert_eq!(status, StatusCode::OK);
        let mut expected = vec![0u8; 10];
        expected.extend_from_slice(b"XXXX");
        assert_eq!(body, expected);
    }
}
