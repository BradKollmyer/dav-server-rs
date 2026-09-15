#![cfg(all(feature = "warp-compat", feature = "memfs"))]

use dav_server::warp::dav_handler;
use dav_server::{DavHandler, fakels::FakeLs, memfs::MemFs};

fn handler() -> DavHandler {
    DavHandler::builder()
        .filesystem(MemFs::new())
        .locksystem(FakeLs::new())
        .build_handler()
}

#[tokio::test]
async fn put_and_get_through_warp() {
    let filter = dav_handler(handler());

    let resp = warp::test::request()
        .method("PUT")
        .path("/notes.txt")
        .body("hello")
        .reply(&filter)
        .await;
    assert_eq!(resp.status(), 201);

    let resp = warp::test::request()
        .method("GET")
        .path("/notes.txt")
        .reply(&filter)
        .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.body(), "hello");
}

#[tokio::test]
async fn get_with_query_string_hits_same_resource() {
    let filter = dav_handler(handler());

    let resp = warp::test::request()
        .method("PUT")
        .path("/notes.txt")
        .body("hello")
        .reply(&filter)
        .await;
    assert_eq!(resp.status(), 201);

    let resp = warp::test::request()
        .method("GET")
        .path("/notes.txt?foo=1")
        .reply(&filter)
        .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.body(), "hello");
}
