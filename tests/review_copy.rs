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

#[tokio::test]
async fn recursive_copy_omits_hidden_entries_without_following_them() {
    let (s, root) = local(false);
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::create_dir(root.join("secret-dir")).unwrap();
    std::fs::write(root.join("secret-dir/key"), "secret").unwrap();
    std::fs::write(root.join(".secret"), "secret").unwrap();
    std::fs::write(root.join("src/.hidden"), "hidden").unwrap();
    std::fs::write(root.join("src/public"), "public").unwrap();
    std::os::unix::fs::symlink("../.secret", root.join("src/leak")).unwrap();
    std::os::unix::fs::symlink("../secret-dir", root.join("src/leak-dir")).unwrap();
    assert_eq!(
        req(&s, "COPY", "/src", "", &[("Destination", "/copy")])
            .await
            .0,
        StatusCode::CREATED
    );
    assert!(!root.join("copy/leak").exists());
    assert!(!root.join("copy/leak-dir").exists());
    assert!(!root.join("copy/.hidden").exists());
    assert_eq!(std::fs::read(root.join("copy/public")).unwrap(), b"public");
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(all(feature = "caldav", feature = "carddav"))]
const ICS: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Test//EN\r\nBEGIN:VEVENT\r\nUID:one\r\nDTSTART:20260101T120000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
#[cfg(all(feature = "caldav", feature = "carddav"))]
const VCARD: &str = "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:one\r\nFN:Alice\r\nEND:VCARD\r\n";

#[cfg(all(feature = "caldav", feature = "carddav"))]
#[tokio::test]
async fn typed_copy_and_move_validate_before_overwrite() {
    for create in ["MKCALENDAR", "MKADDRESSBOOK"] {
        let content = if create == "MKCALENDAR" { ICS } else { VCARD };
        for method in ["COPY", "MOVE"] {
            let s = DavHandler::builder()
                .filesystem(dav_server::memfs::MemFs::new())
                .build_handler();
            assert_eq!(
                req(&s, create, "/typed", "", &[]).await.0,
                StatusCode::CREATED
            );
            assert_eq!(
                req(&s, "PUT", "/typed/object", content, &[]).await.0,
                StatusCode::CREATED
            );
            let oversized = "x".repeat(1024 * 1024 + 1);
            for rejected in ["invalid", &oversized] {
                assert!(req(&s, "PUT", "/plain", rejected, &[]).await.0.is_success());
                for dest in ["/typed/new", "/typed/object"] {
                    assert_eq!(
                        req(&s, method, "/plain", "", &[("Destination", dest)])
                            .await
                            .0,
                        StatusCode::FORBIDDEN
                    );
                    assert_eq!(
                        req(&s, "GET", "/typed/object", "", &[]).await,
                        (StatusCode::OK, content.to_owned())
                    );
                    assert_eq!(
                        req(&s, "GET", "/typed/new", "", &[]).await.0,
                        StatusCode::NOT_FOUND
                    );
                    assert_eq!(
                        req(&s, "GET", "/plain", "", &[]).await,
                        (StatusCode::OK, rejected.to_owned())
                    );
                }
            }
            assert_eq!(
                req(&s, "MKCOL", "/dir", "", &[]).await.0,
                StatusCode::CREATED
            );
            assert_eq!(
                req(&s, method, "/dir", "", &[("Destination", "/typed/object")])
                    .await
                    .0,
                StatusCode::FORBIDDEN
            );
            assert!(req(&s, "PUT", "/plain", content, &[]).await.0.is_success());
            assert_eq!(
                req(
                    &s,
                    method,
                    "/plain",
                    "",
                    &[("Destination", "/typed/object")]
                )
                .await
                .0,
                StatusCode::NO_CONTENT
            );
            assert_eq!(
                req(&s, "GET", "/typed/object", "", &[]).await,
                (StatusCode::OK, content.to_owned())
            );
            assert_eq!(
                req(&s, "GET", "/plain", "", &[]).await.0,
                if method == "COPY" {
                    StatusCode::OK
                } else {
                    StatusCode::NOT_FOUND
                }
            );
        }
    }
}

#[cfg(all(feature = "caldav", feature = "carddav", feature = "proppatch"))]
#[tokio::test]
async fn collection_copy_preserves_type_and_properties_at_both_depths() {
    for create in ["MKCALENDAR", "MKADDRESSBOOK"] {
        let content = if create == "MKCALENDAR" { ICS } else { VCARD };
        for depth in ["0", "infinity"] {
            let s = DavHandler::builder()
                .filesystem(dav_server::memfs::MemFs::new())
                .build_handler();
            assert_eq!(
                req(&s, create, "/typed", "", &[]).await.0,
                StatusCode::CREATED
            );
            assert_eq!(
                req(&s, "PUT", "/typed/object", content, &[]).await.0,
                StatusCode::CREATED
            );
            let prop = r#"<D:propertyupdate xmlns:D="DAV:" xmlns:T="urn:test"><D:set><D:prop><T:color>blue</T:color></D:prop></D:set></D:propertyupdate>"#;
            assert_eq!(
                req(&s, "PROPPATCH", "/typed", prop, &[]).await.0,
                StatusCode::MULTI_STATUS
            );
            assert_eq!(
                req(
                    &s,
                    "COPY",
                    "/typed",
                    "",
                    &[("Destination", "/copy"), ("Depth", depth)]
                )
                .await
                .0,
                StatusCode::CREATED
            );
            let props = req(&s, "PROPFIND", "/copy", r#"<D:propfind xmlns:D="DAV:" xmlns:T="urn:test"><D:prop><T:color/></D:prop></D:propfind>"#, &[("Depth", "0")]).await;
            assert!(props.1.contains("blue"), "{}", props.1);
            // Validate through behavior: the copied metadata flag must still
            // enforce the collection's object format.
            assert_eq!(
                req(&s, "PUT", "/copy/invalid", "invalid", &[]).await.0,
                StatusCode::FORBIDDEN
            );
            assert_eq!(
                req(&s, "GET", "/copy/object", "", &[]).await.0,
                if depth == "0" {
                    StatusCode::NOT_FOUND
                } else {
                    StatusCode::OK
                }
            );
        }
    }
}

#[cfg(all(feature = "caldav", feature = "carddav"))]
#[tokio::test]
async fn local_collection_copy_preserves_type_when_sidecars_are_hidden() {
    for create in ["MKCALENDAR", "MKADDRESSBOOK"] {
        let (s, root) = local(false);
        assert_eq!(
            req(&s, create, "/typed", "", &[]).await.0,
            StatusCode::CREATED
        );
        for depth in ["0", "infinity"] {
            assert!(
                req(
                    &s,
                    "COPY",
                    "/typed",
                    "",
                    &[("Destination", "/copy"), ("Depth", depth)]
                )
                .await
                .0
                .is_success()
            );
            assert_eq!(
                req(&s, "PUT", "/copy/invalid", "invalid", &[]).await.0,
                StatusCode::FORBIDDEN
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(all(feature = "caldav", feature = "carddav"))]
#[tokio::test]
async fn depth_zero_copy_preserves_existing_collection_type_and_members() {
    for use_local in [false, true] {
        for (source_method, source_data) in [
            ("MKCOL", "plain"),
            ("MKCALENDAR", ICS),
            ("MKADDRESSBOOK", VCARD),
        ] {
            for (dest_method, dest_data) in [
                ("MKCOL", "plain"),
                ("MKCALENDAR", ICS),
                ("MKADDRESSBOOK", VCARD),
            ] {
                let (server, root) = if use_local {
                    let (server, root) = local(false);
                    (server, Some(root))
                } else {
                    (
                        DavHandler::builder()
                            .filesystem(dav_server::memfs::MemFs::new())
                            .build_handler(),
                        None,
                    )
                };
                for (method, path, data) in [
                    (source_method, "/source", source_data),
                    (dest_method, "/dest", dest_data),
                ] {
                    assert_eq!(
                        req(&server, method, path, "", &[]).await.0,
                        StatusCode::CREATED
                    );
                    assert_eq!(
                        req(&server, "PUT", &format!("{path}/item"), data, &[])
                            .await
                            .0,
                        StatusCode::CREATED
                    );
                }
                let props = r#"<D:propfind xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav" xmlns:CARD="urn:ietf:params:xml:ns:carddav"><D:prop><D:resourcetype/></D:prop></D:propfind>"#;
                let before = req(&server, "PROPFIND", "/dest", props, &[("Depth", "0")]).await;
                let copied = req(
                    &server,
                    "COPY",
                    "/source",
                    "",
                    &[("Destination", "/dest"), ("Depth", "0")],
                )
                .await;
                assert_eq!(
                    copied.0,
                    if source_method == dest_method {
                        StatusCode::NO_CONTENT
                    } else {
                        StatusCode::FORBIDDEN
                    },
                    "{source_method} -> {dest_method}, local={use_local}: {copied:?}"
                );
                assert_eq!(
                    req(&server, "PROPFIND", "/dest", props, &[("Depth", "0")]).await,
                    before
                );
                assert_eq!(
                    req(&server, "GET", "/dest/item", "", &[]).await,
                    (StatusCode::OK, dest_data.to_owned())
                );
                if let Some(root) = root {
                    std::fs::remove_dir_all(root).unwrap();
                }
            }
        }
    }
}
