#![cfg(all(feature = "actix-compat", feature = "memfs"))]

use actix_web::{App, http::StatusCode, test, web};
use dav_server::actix::{DavRequest, DavResponse};
use dav_server::{DavHandler, fakels::FakeLs, memfs::MemFs};

async fn dav_handler(req: DavRequest, davhandler: web::Data<DavHandler>) -> DavResponse {
    davhandler.handle(req.request).await.into()
}

fn handler() -> DavHandler {
    DavHandler::builder()
        .filesystem(MemFs::new())
        .locksystem(FakeLs::new())
        .build_handler()
}

#[actix_web::test]
async fn put_and_get_through_actix() {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(handler()))
            .service(web::resource("/{tail:.*}").to(dav_handler)),
    )
    .await;

    let req = test::TestRequest::put()
        .uri("/notes.txt")
        .set_payload("hello")
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::CREATED);

    let req = test::TestRequest::get().uri("/notes.txt").to_request();
    let resp = test::call_service(&app, req).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = test::read_body(resp).await;
    assert_eq!(&body[..], b"hello");
}
