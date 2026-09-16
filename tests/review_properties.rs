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

#[tokio::test]
async fn carddav_report_omits_empty_200_propstat() {
    let server = DavHandler::builder()
        .filesystem(dav_server::memfs::MemFs::new())
        .build_handler();
    assert_eq!(
        request(&server, "MKADDRESSBOOK", "/people", "").await.0,
        StatusCode::CREATED
    );
    assert_eq!(
        request(&server, "PUT", "/people/a.vcf", VCARD).await.0,
        StatusCode::CREATED
    );
    let query = r#"<T:addressbook-query xmlns:T="urn:ietf:params:xml:ns:carddav" xmlns:D="DAV:"><D:prop><D:displayname/></D:prop></T:addressbook-query>"#;
    let (status, xml) = request(&server, "REPORT", "/people", query).await;
    assert_eq!(status, StatusCode::MULTI_STATUS, "{xml}");
    let tree = xmltree::Element::parse(xml.as_bytes()).unwrap();
    let response = tree.get_child("response").expect(xml.as_str());
    assert!(
        response
            .get_child("href")
            .unwrap()
            .get_text()
            .unwrap()
            .contains("/people/a.vcf"),
        "{xml}"
    );
    for child in &response.children {
        if let xmltree::XMLNode::Element(elem) = child
            && elem.name == "propstat"
        {
            let st = elem.get_child("status").unwrap().get_text().unwrap();
            assert!(
                !st.contains("200"),
                "empty 200 propstat must not be emitted: {xml}"
            );
        }
    }
}

#[cfg(feature = "proppatch")]
fn creation_cases(property: &str) -> Vec<(&'static str, String)> {
    [
        ("MKCALENDAR", "C:mkcalendar", "<D:collection/><C:calendar/>"),
        ("MKCOL", "D:mkcol", "<D:collection/><C:calendar/>"),
        ("MKCOL", "D:mkcol", "<D:collection/><CARD:addressbook/>"),
        ("MKADDRESSBOOK", "CARD:mkaddressbook", "<D:collection/><CARD:addressbook/>"),
    ].into_iter().map(|(method, root, resource_type)| (method, format!(r#"<{root} xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav" xmlns:CARD="urn:ietf:params:xml:ns:carddav"><D:set><D:prop><D:resourcetype>{resource_type}</D:resourcetype>{property}</D:prop></D:set></{root}>"#))).collect()
}

#[cfg(feature = "proppatch")]
async fn assert_creation_rolled_back(server: &DavHandler, property: &str, expected: StatusCode) {
    for (index, (method, body)) in creation_cases(property).into_iter().enumerate() {
        let path = format!("/failed{index}");
        let result = request(server, method, &path, &body).await;
        assert_eq!(result.0, expected, "{method}: {result:?}");
        assert_eq!(
            request(server, "PROPFIND", &path, "").await.0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            request(server, method, &path, "").await.0,
            StatusCode::CREATED
        );
    }
}

#[cfg(feature = "proppatch")]
#[tokio::test]
async fn protected_properties_roll_back_typed_collection_creation() {
    let server = DavHandler::builder()
        .filesystem(dav_server::memfs::MemFs::new())
        .build_handler();
    assert_creation_rolled_back(
        &server,
        "<D:getetag>forbidden</D:getetag>",
        StatusCode::FORBIDDEN,
    )
    .await;
}

#[cfg(all(feature = "proppatch", feature = "localfs"))]
#[tokio::test]
async fn localfs_failed_creation_removes_type_marker_sidecars() {
    let root = std::env::temp_dir().join(format!("dav-rollback-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    let server = DavHandler::builder()
        .filesystem(dav_server::localfs::LocalFs::new(
            &root, false, false, false,
        ))
        .build_handler();
    assert_creation_rolled_back(
        &server,
        "<D:getetag>forbidden</D:getetag>",
        StatusCode::FORBIDDEN,
    )
    .await;
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(feature = "proppatch")]
mod backend_failures {
    use super::*;
    use dav_server::{
        davpath::DavPath,
        fs::{
            DavDirEntry, DavFile, DavFileSystem, DavMetaData, DavProp, FsError, FsFuture, FsStream,
            OpenOptions, ReadDirMeta,
        },
    };

    #[derive(Clone)]
    struct RefusingFs(dav_server::memfs::MemFs, bool);

    impl DavFileSystem for RefusingFs {
        fn open<'a>(
            &'a self,
            path: &'a DavPath,
            options: OpenOptions,
        ) -> FsFuture<'a, Box<dyn DavFile>> {
            self.0.open(path, options)
        }
        fn read_dir<'a>(
            &'a self,
            path: &'a DavPath,
            meta: ReadDirMeta,
        ) -> FsFuture<'a, FsStream<Box<dyn DavDirEntry>>> {
            self.0.read_dir(path, meta)
        }
        fn metadata<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, Box<dyn DavMetaData>> {
            self.0.metadata(path)
        }
        fn symlink_metadata<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, Box<dyn DavMetaData>> {
            self.0.symlink_metadata(path)
        }
        fn create_dir<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, ()> {
            self.0.create_dir(path)
        }
        fn remove_dir<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, ()> {
            self.0.remove_dir(path)
        }
        fn have_props<'a>(
            &'a self,
            path: &'a DavPath,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + 'a>> {
            self.0.have_props(path)
        }
        fn mark_calendar<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, ()> {
            self.0.mark_calendar(path)
        }
        fn mark_addressbook<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, ()> {
            self.0.mark_addressbook(path)
        }
        fn patch_props<'a>(
            &'a self,
            _path: &'a DavPath,
            patch: Vec<(bool, DavProp)>,
        ) -> FsFuture<'a, Vec<(StatusCode, DavProp)>> {
            Box::pin(async move {
                if self.1 {
                    Err(FsError::InsufficientStorage)
                } else {
                    Ok(patch
                        .into_iter()
                        .map(|(_, prop)| (StatusCode::FORBIDDEN, prop))
                        .collect())
                }
            })
        }
    }

    #[tokio::test]
    async fn refused_property_status_rolls_back_creation() {
        let server = DavHandler::builder()
            .filesystem(Box::new(RefusingFs(
                *dav_server::memfs::MemFs::new(),
                false,
            )))
            .build_handler();
        assert_creation_rolled_back(
            &server,
            "<D:displayname>New collection</D:displayname>",
            StatusCode::FORBIDDEN,
        )
        .await;
    }

    #[tokio::test]
    async fn property_backend_error_rolls_back_creation() {
        let server = DavHandler::builder()
            .filesystem(Box::new(RefusingFs(*dav_server::memfs::MemFs::new(), true)))
            .build_handler();
        assert_creation_rolled_back(
            &server,
            "<D:displayname>New collection</D:displayname>",
            StatusCode::INSUFFICIENT_STORAGE,
        )
        .await;
    }
}
