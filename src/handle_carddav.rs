use futures_util::StreamExt;
use headers::HeaderMapExt;
use http::{Request, Response, StatusCode};
use std::io::Cursor;
use xmltree::{Element, XMLNode};

use crate::body::Body;
use crate::errors::*;
use crate::xmltree_ext::ElementExt;
use crate::{DavInner, DavResult};

use crate::async_stream::AsyncStream;
use crate::carddav::*;
use crate::dav_filters::hrefs_from;
use crate::davheaders;
use crate::davpath::DavPath;
use crate::handle_props::PropWriter;

enum ParsedCardDavReportType {
    AddressBookQuery(ParsedAddressBookQuery),
    AddressBookMultiget { hrefs: Vec<String> },
}

fn filter_test(elem: &Element) -> DavResult<FilterTest> {
    match elem.attributes.get("test").map(String::as_str) {
        None | Some("anyof") => Ok(FilterTest::AnyOf),
        Some("allof") => Ok(FilterTest::AllOf),
        _ => Err(DavError::Status(StatusCode::BAD_REQUEST)),
    }
}

impl<C: Clone + Send + Sync + 'static> DavInner<C> {
    /// Handle REPORT method when only CardDAV is enabled (not CalDAV)
    ///
    /// When CalDAV is also enabled, the unified handle_report in handle_caldav.rs
    /// is used instead, which delegates to handle_carddav_report for CardDAV requests.
    #[cfg(not(feature = "caldav"))]
    pub(crate) async fn handle_report(
        &self,
        req: &Request<()>,
        body: &[u8],
    ) -> DavResult<Response<Body>> {
        let root = Element::parse2(Cursor::new(body))?;
        self.handle_carddav_report(req, &root).await
    }

    /// Handle CardDAV REPORT method for addressbook-query and addressbook-multiget
    ///
    /// `root` is the already parsed root element of the REPORT request body.
    pub(crate) async fn handle_carddav_report(
        &self,
        req: &Request<()>,
        root: &Element,
    ) -> DavResult<Response<Body>> {
        let path = self.path(req);
        self.ensure_visible(&path).await?;

        let report_type = self.parse_carddav_report_request(root)?;

        // RFC 6352 8.6-8.7: these reports are scoped to address book
        // collections, and supported-report-set only advertises them there.
        // addressbook-query and addressbook-multiget may also be addressed
        // to an address object resource inside an address book.
        let meta = self.fs.metadata(&path, &self.credentials).await?;
        let target_is_addressbook = if meta.is_dir() {
            self.collection_is_addressbook(&path, meta.as_ref()).await
        } else {
            let parent = path.parent();
            match self.fs.metadata(&parent, &self.credentials).await {
                Ok(pmeta) => {
                    self.collection_is_addressbook(&parent, pmeta.as_ref())
                        .await
                }
                Err(_) => false,
            }
        };
        if !target_is_addressbook {
            return Err(DavError::Status(StatusCode::FORBIDDEN));
        }

        match report_type {
            ParsedCardDavReportType::AddressBookQuery(query) => {
                // RFC 6352 8.6: when no Depth header is present the query
                // applies to the address book collection's immediate members, as
                // if Depth: 1 had been sent. Depth: 0 targets the collection
                // resource itself (no match for a query).
                let depth = req
                    .headers()
                    .typed_try_get::<davheaders::Depth>()
                    .map_err(|_| DavError::Status(StatusCode::BAD_REQUEST))?
                    .unwrap_or(davheaders::Depth::One);
                self.handle_addressbook_query(&path, meta.is_dir(), depth, query)
                    .await
            }
            ParsedCardDavReportType::AddressBookMultiget { hrefs } => {
                self.handle_addressbook_multiget(&path, hrefs).await
            }
        }
    }

    /// Handle CardDAV MKADDRESSBOOK method.
    ///
    /// RFC 6352 creates address books with extended MKCOL. MKADDRESSBOOK is a
    /// compatibility alias with the same parent/If/lock/409/405 behaviour as MKCOL.
    pub(crate) async fn handle_mkaddressbook(
        &self,
        req: &Request<()>,
        body: &[u8],
    ) -> DavResult<Response<Body>> {
        let tree = if body.is_empty() {
            None
        } else {
            let tree = Element::parse2(Cursor::new(body))?;
            if tree.name != "mkcol" && tree.name != "mkaddressbook" {
                return Err(DavError::XmlParseError);
            }
            Some(tree)
        };
        let path = self.path(req);
        let parent = path.parent();
        if let Ok(meta) = self.fs.metadata(&parent, &self.credentials).await
            && self.collection_is_addressbook(&parent, meta.as_ref()).await
        {
            return Err(DavError::Status(StatusCode::FORBIDDEN));
        }
        let resp = self.handle_mkcol(req, &[]).await?;
        if let Err(e) = self.fs.mark_addressbook(&path, &self.credentials).await {
            self.rollback_created_collection(&path).await;
            return Err(e.into());
        }
        if let Some(tree) = tree {
            #[cfg(feature = "proppatch")]
            if let Err(e) = self.apply_set_props(&path, &tree).await {
                self.rollback_created_collection(&path).await;
                return Err(e);
            }
            #[cfg(not(feature = "proppatch"))]
            let _ = tree;
        }
        Ok(resp)
    }

    fn parse_carddav_report_request(&self, root: &Element) -> DavResult<ParsedCardDavReportType> {
        match root.name.as_str() {
            "addressbook-query" => {
                let query = self.parse_addressbook_query(root)?;
                Ok(ParsedCardDavReportType::AddressBookQuery(query))
            }
            "addressbook-multiget" => {
                let hrefs = hrefs_from(root);
                Ok(ParsedCardDavReportType::AddressBookMultiget { hrefs })
            }
            _ => Err(DavError::StatusClose(StatusCode::BAD_REQUEST)),
        }
    }

    fn parse_addressbook_query(&self, root: &Element) -> DavResult<ParsedAddressBookQuery> {
        let mut query = ParsedAddressBookQuery {
            query: AddressBookQuery {
                prop_filter: None,
                properties: Vec::new(),
                limit: None,
            },
            filters: Vec::new(),
            test: FilterTest::AnyOf,
        };
        let mut have_filter = false;

        for child in &root.children {
            if let XMLNode::Element(elem) = child {
                match elem.name.as_str() {
                    "filter" => {
                        if have_filter {
                            return Err(DavError::Status(StatusCode::BAD_REQUEST));
                        }
                        have_filter = true;
                        query.test = filter_test(elem)?;
                        // Retain every property predicate in the filter.
                        for filter_child in &elem.children {
                            if let XMLNode::Element(filter_elem) = filter_child
                                && filter_elem.name == "prop-filter"
                            {
                                query
                                    .filters
                                    .push(self.parse_carddav_property_filter(filter_elem)?);
                            }
                        }
                    }
                    "prop" => {
                        // Parse requested properties
                        for prop_child in &elem.children {
                            if let XMLNode::Element(prop_elem) = prop_child {
                                query.query.properties.push(prop_elem.name.clone());
                            }
                        }
                    }
                    "limit" => {
                        // Parse limit element
                        for limit_child in &elem.children {
                            if let XMLNode::Element(limit_elem) = limit_child
                                && limit_elem.name == "nresults"
                                && let Some(text) = limit_elem.children.iter().find_map(|c| {
                                    if let XMLNode::Text(t) = c {
                                        Some(t)
                                    } else {
                                        None
                                    }
                                })
                            {
                                query.query.limit = text.parse().ok();
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        Ok(query)
    }

    fn parse_carddav_property_filter(&self, elem: &Element) -> DavResult<ParsedPropertyFilter> {
        let name = elem
            .attributes
            .get("name")
            .ok_or(DavError::StatusClose(StatusCode::BAD_REQUEST))?
            .clone();

        let mut filter = ParsedPropertyFilter {
            name,
            is_not_defined: false,
            text_matches: Vec::new(),
            param_filters: Vec::new(),
            test: filter_test(elem)?,
        };

        for child in &elem.children {
            if let XMLNode::Element(child_elem) = child {
                match child_elem.name.as_str() {
                    "is-not-defined" => {
                        filter.is_not_defined = true;
                    }
                    "text-match" => {
                        filter
                            .text_matches
                            .push(TextMatch::from_element(child_elem));
                    }
                    "param-filter" => {
                        filter
                            .param_filters
                            .push(ParameterFilter::from_element(child_elem)?);
                    }
                    _ => {}
                }
            }
        }

        if filter.is_not_defined
            && (!filter.text_matches.is_empty() || !filter.param_filters.is_empty())
        {
            return Err(DavError::Status(StatusCode::BAD_REQUEST));
        }
        Ok(filter)
    }

    async fn handle_addressbook_query(
        &self,
        path: &DavPath,
        is_collection: bool,
        depth: davheaders::Depth,
        query: ParsedAddressBookQuery,
    ) -> DavResult<Response<Body>> {
        let mut paths = Vec::new();
        if !is_collection {
            // An object-targeted query evaluates that object, never its siblings.
            paths.push(path.clone());
        } else if depth != davheaders::Depth::Zero {
            let mut stream = self
                .fs
                .read_dir(path, self.get_read_dir_meta(), &self.credentials)
                .await?;
            while let Some(item) = stream.next().await {
                let Ok(dirent) = item else { continue };
                if self.hidden_in_listing(dirent.as_ref()).await {
                    continue;
                }
                let mut item_path = path.clone();
                item_path.push_segment(&dirent.name());
                paths.push(item_path);
            }
        }
        let mut results = Vec::new();
        let mut count = 0u32;
        for item_path in paths {
            // Check limit
            if let Some(limit) = query.query.limit
                && count >= limit
            {
                break;
            }

            // Check if this is a vCard resource, and append content to result
            if let Some((metadata, content)) = self
                .read_object_resource(&item_path, DEFAULT_MAX_RESOURCE_SIZE, is_vcard_data)
                .await
                && addressbook_matches_parsed_query(&content, &query)
            {
                let etag = crate::davheaders::ETag::from_meta(metadata.as_ref())
                    .map(|etag| etag.to_string())
                    .unwrap_or_default();
                results.push((item_path, etag, content));
                count += 1;
            }
        }

        // Generate multistatus response, honoring the requested prop set.
        self.generate_addressbook_multiget_response(
            results,
            Vec::new(),
            query.query.properties.clone(),
        )
        .await
    }

    async fn handle_addressbook_multiget(
        &self,
        path: &DavPath,
        hrefs: Vec<String>,
    ) -> DavResult<Response<Body>> {
        let mut results = Vec::new();
        let mut missing_hrefs: Vec<String> = Vec::new();

        for href in &hrefs {
            // RFC 6352 8.7: hrefs must be address object resources in this
            // collection, or the request URI itself when the REPORT is
            // addressed to an address object resource.
            if let Ok(item_path) = DavPath::from_str_and_prefix(href, &self.prefix)
                && (item_path.is_in_collection(path) || item_path == *path)
                && self.ensure_visible(&item_path).await.is_ok()
                && let Some((metadata, content)) = self
                    .read_object_resource(&item_path, DEFAULT_MAX_RESOURCE_SIZE, is_vcard_data)
                    .await
            {
                let etag = crate::davheaders::ETag::from_meta(metadata.as_ref())
                    .map(|etag| etag.to_string())
                    .unwrap_or_default();
                results.push((item_path, etag, content));
                continue;
            }

            missing_hrefs.push(href.clone());
        }

        self.generate_addressbook_multiget_response(results, missing_hrefs, Vec::new())
            .await
    }

    #[cfg(feature = "carddav")]
    async fn generate_addressbook_multiget_response(
        &self,
        results: Vec<(DavPath, String, String)>,
        missing_hrefs: Vec<String>,
        requested_props: Vec<String>,
    ) -> DavResult<Response<Body>> {
        let mut resp = Response::new(Body::empty());

        // Create a minimal request for PropWriter
        let req = http::Request::builder()
            .method(http::Method::GET)
            .uri("/")
            .body(())
            .unwrap();

        let empty_path = DavPath::new("/").unwrap();

        let mut pw = PropWriter::new(
            &req,
            &mut resp,
            "prop",
            Vec::new(),
            self.fs.clone(),
            self.ls.as_ref(),
            self.principal.clone(),
            self.credentials.clone(),
            &empty_path,
        )?;

        *resp.body_mut() = Body::from(AsyncStream::new(|tx| async move {
            pw.set_tx(tx);

            for (href, etag, vcard_data) in results {
                pw.write_vcard_data_response(&href, &etag, &vcard_data, &requested_props)?;
            }

            for missing_href in missing_hrefs {
                pw.write_vcard_not_found_response(&missing_href)?;
            }

            pw.close().await?;

            Ok(())
        }));

        Ok(resp)
    }
}
