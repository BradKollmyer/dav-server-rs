use futures_util::StreamExt;
use http::{Request, Response, StatusCode};
use std::io::Cursor;
use xml::reader::{EventReader, XmlEvent};
use xmltree::{Element, XMLNode};

use crate::body::Body;
use crate::errors::*;
use crate::fs::*;
use crate::xmltree_ext::ElementExt;
use crate::{DavInner, DavResult};

use crate::async_stream::AsyncStream;
use crate::carddav::*;
use crate::davpath::DavPath;
use crate::handle_props::PropWriter;

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
        self.handle_carddav_report(req, body).await
    }

    /// Handle CardDAV REPORT method for addressbook-query and addressbook-multiget
    pub(crate) async fn handle_carddav_report(
        &self,
        req: &Request<()>,
        body: &[u8],
    ) -> DavResult<Response<Body>> {
        let path = self.path(req);

        // Parse the REPORT request body
        let report_type = self.parse_carddav_report_request(body)?;

        match report_type {
            CardDavReportType::AddressBookQuery(query) => {
                self.handle_addressbook_query(&path, query).await
            }
            CardDavReportType::AddressBookMultiget { hrefs } => {
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
        let set_props = parse_mkcol_or_mkaddressbook_set_props(body)?;
        let resp = self.handle_mkcol(req).await?;
        self.apply_mkcol_set_props(&self.path(req), set_props).await;
        Ok(resp)
    }

    async fn apply_mkcol_set_props(&self, path: &DavPath, props: Vec<DavProp>) {
        #[cfg(feature = "proppatch")]
        if !props.is_empty() && self.fs.have_props(path, &self.credentials).await {
            let patch = props.into_iter().map(|p| (true, p)).collect();
            let _ = self.fs.patch_props(path, patch, &self.credentials).await;
        }
        #[cfg(not(feature = "proppatch"))]
        {
            let _ = (path, props);
        }
    }

    fn parse_carddav_report_request(&self, body: &[u8]) -> DavResult<CardDavReportType> {
        if body.is_empty() {
            return Err(DavError::StatusClose(StatusCode::BAD_REQUEST));
        }

        let cursor = Cursor::new(body);
        let parser = EventReader::new(cursor);
        let mut elements: Vec<Element> = Vec::new();
        let mut current_element: Option<Element> = None;
        let mut element_stack: Vec<Element> = Vec::new();

        for event in parser {
            match event {
                Ok(XmlEvent::StartElement {
                    name,
                    attributes,
                    namespace,
                }) => {
                    let mut elem = Element::new(&name.local_name);
                    if let Some(prefix) = name.prefix
                        && let Some(uri) = namespace.get(&prefix)
                    {
                        elem.namespace = Some(uri.to_string());
                    }

                    for attr in attributes {
                        elem.attributes.insert(attr.name.local_name, attr.value);
                    }

                    if let Some(parent) = current_element.take() {
                        element_stack.push(parent);
                    }
                    current_element = Some(elem);
                }
                Ok(XmlEvent::EndElement { .. }) => {
                    if let Some(elem) = current_element.take() {
                        if let Some(mut parent) = element_stack.pop() {
                            parent.children.push(XMLNode::Element(elem));
                            current_element = Some(parent);
                        } else {
                            elements.push(elem);
                        }
                    }
                }
                Ok(XmlEvent::Characters(text)) => {
                    if let Some(ref mut elem) = current_element {
                        elem.children.push(XMLNode::Text(text));
                    }
                }
                _ => {}
            }
        }

        // Parse the root element to determine report type
        if let Some(root) = elements.first() {
            match root.name.as_str() {
                "addressbook-query" => {
                    let query = self.parse_addressbook_query(root)?;
                    Ok(CardDavReportType::AddressBookQuery(query))
                }
                "addressbook-multiget" => {
                    let hrefs = self.parse_addressbook_multiget(root)?;
                    Ok(CardDavReportType::AddressBookMultiget { hrefs })
                }
                _ => Err(DavError::StatusClose(StatusCode::BAD_REQUEST)),
            }
        } else {
            Err(DavError::StatusClose(StatusCode::BAD_REQUEST))
        }
    }

    fn parse_addressbook_query(&self, root: &Element) -> DavResult<AddressBookQuery> {
        let mut query = AddressBookQuery {
            prop_filter: None,
            properties: Vec::new(),
            limit: None,
        };

        for child in &root.children {
            if let XMLNode::Element(elem) = child {
                match elem.name.as_str() {
                    "filter" => {
                        // Parse prop-filter elements
                        for filter_child in &elem.children {
                            if let XMLNode::Element(filter_elem) = filter_child
                                && filter_elem.name == "prop-filter"
                            {
                                query.prop_filter =
                                    Some(self.parse_carddav_property_filter(filter_elem)?);
                            }
                        }
                    }
                    "prop" => {
                        // Parse requested properties
                        for prop_child in &elem.children {
                            if let XMLNode::Element(prop_elem) = prop_child {
                                query.properties.push(prop_elem.name.clone());
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
                                query.limit = text.parse().ok();
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        Ok(query)
    }

    fn parse_carddav_property_filter(&self, elem: &Element) -> DavResult<PropertyFilter> {
        let name = elem
            .attributes
            .get("name")
            .ok_or(DavError::StatusClose(StatusCode::BAD_REQUEST))?
            .clone();

        let mut filter = PropertyFilter {
            name,
            is_not_defined: false,
            text_match: None,
            param_filters: Vec::new(),
        };

        for child in &elem.children {
            if let XMLNode::Element(child_elem) = child {
                match child_elem.name.as_str() {
                    "is-not-defined" => {
                        filter.is_not_defined = true;
                    }
                    "text-match" => {
                        filter.text_match = Some(self.parse_carddav_text_match(child_elem)?);
                    }
                    "param-filter" => {
                        filter
                            .param_filters
                            .push(self.parse_carddav_param_filter(child_elem)?);
                    }
                    _ => {}
                }
            }
        }

        Ok(filter)
    }

    fn parse_carddav_text_match(&self, elem: &Element) -> DavResult<TextMatch> {
        let text = elem
            .children
            .iter()
            .find_map(|child| {
                if let XMLNode::Text(text) = child {
                    Some(text.clone())
                } else {
                    None
                }
            })
            .unwrap_or_default();

        Ok(TextMatch {
            text,
            collation: elem.attributes.get("collation").cloned(),
            negate_condition: elem
                .attributes
                .get("negate-condition")
                .map(|v| v == "yes")
                .unwrap_or(false),
            match_type: elem.attributes.get("match-type").cloned(),
        })
    }

    fn parse_carddav_param_filter(&self, elem: &Element) -> DavResult<ParameterFilter> {
        let name = elem
            .attributes
            .get("name")
            .ok_or(DavError::StatusClose(StatusCode::BAD_REQUEST))?
            .clone();

        let mut filter = ParameterFilter {
            name,
            is_not_defined: false,
            text_match: None,
        };

        for child in &elem.children {
            if let XMLNode::Element(child_elem) = child {
                match child_elem.name.as_str() {
                    "is-not-defined" => {
                        filter.is_not_defined = true;
                    }
                    "text-match" => {
                        filter.text_match = Some(self.parse_carddav_text_match(child_elem)?);
                    }
                    _ => {}
                }
            }
        }

        Ok(filter)
    }

    fn parse_addressbook_multiget(&self, root: &Element) -> DavResult<Vec<String>> {
        let mut hrefs = Vec::new();

        for child in &root.children {
            if let XMLNode::Element(elem) = child
                && elem.name == "href"
            {
                for href_child in &elem.children {
                    if let XMLNode::Text(href) = href_child {
                        hrefs.push(href.clone());
                    }
                }
            }
        }

        Ok(hrefs)
    }

    async fn handle_addressbook_query(
        &self,
        path: &DavPath,
        query: AddressBookQuery,
    ) -> DavResult<Response<Body>> {
        // Get directory listing
        let stream = self
            .fs
            .read_dir(path, ReadDirMeta::Data, &self.credentials)
            .await?;
        let mut results = Vec::new();

        let items: Vec<_> = stream.collect().await;
        let mut count = 0u32;
        for item in items {
            // Check limit
            if let Some(limit) = query.limit
                && count >= limit
            {
                break;
            }

            match item {
                Ok(dirent) => {
                    let mut item_path = path.clone();
                    item_path.push_segment(&dirent.name());

                    // Check if this is a vCard resource, and append content to result
                    if let Ok(mut file) = self
                        .fs
                        .open(&item_path, OpenOptions::read(), &self.credentials)
                        .await
                        && let Ok(metadata) = file.metadata().await
                        && metadata.len() <= DEFAULT_MAX_RESOURCE_SIZE
                        && let Ok(data) = file.read_bytes(metadata.len() as usize).await
                        && is_vcard_data(&data)
                    {
                        let content = String::from_utf8_lossy(&data);

                        if self.matches_addressbook_query(&content, &query) {
                            let etag = metadata.etag().unwrap_or_default().to_string();
                            results.push((item_path.clone(), etag, content.to_string()));
                            count += 1;
                            continue;
                        }
                    }
                }
                Err(_) => continue,
            }
        }

        // Generate multistatus response
        self.generate_addressbook_multiget_response(results, Vec::new())
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
            // RFC 6352 6.3: hrefs must be address-object resources in this collection.
            if let Ok(item_path) = DavPath::from_str_and_prefix(href, &self.prefix)
                && item_path.is_in_collection(path)
                && let Ok(mut file) = self
                    .fs
                    .open(&item_path, OpenOptions::read(), &self.credentials)
                    .await
                && let Ok(metadata) = file.metadata().await
                && metadata.len() <= DEFAULT_MAX_RESOURCE_SIZE
                && let Ok(data) = file.read_bytes(metadata.len() as usize).await
                && is_vcard_data(&data)
            {
                let etag = metadata.etag().unwrap_or_default().to_string();
                let content = String::from_utf8_lossy(&data);
                results.push((item_path, etag, content.to_string()));
                continue;
            }

            missing_hrefs.push(href.clone());
        }

        self.generate_addressbook_multiget_response(results, missing_hrefs)
            .await
    }

    fn matches_addressbook_query(&self, content: &str, query: &AddressBookQuery) -> bool {
        addressbook_matches_query(content, query)
    }

    #[cfg(feature = "carddav")]
    async fn generate_addressbook_multiget_response(
        &self,
        results: Vec<(DavPath, String, String)>,
        missing_hrefs: Vec<String>,
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
                pw.write_vcard_data_response(&href, &etag, &vcard_data)?;
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

/// Empty body: no props. Otherwise the root must be `mkcol` or `mkaddressbook`.
fn parse_mkcol_or_mkaddressbook_set_props(body: &[u8]) -> DavResult<Vec<DavProp>> {
    if body.is_empty() {
        return Ok(Vec::new());
    }

    let tree = Element::parse2(Cursor::new(body))?;
    if tree.name != "mkcol" && tree.name != "mkaddressbook" {
        return Err(DavError::XmlParseError);
    }

    let mut props = Vec::new();
    for set in tree.child_elems_iter() {
        if set.name != "set" {
            continue;
        }
        for prop_elem in set.child_elems_iter() {
            if prop_elem.name != "prop" {
                continue;
            }
            for n in prop_elem.child_elems_iter() {
                if n.name == "resourcetype" {
                    continue;
                }
                props.push(element_to_davprop_full(n));
            }
        }
    }
    Ok(props)
}

fn element_to_davprop_full(elem: &Element) -> DavProp {
    let mut emitter = xml::writer::EventWriter::new(Cursor::new(Vec::new()));
    elem.write_ev(&mut emitter).ok();
    let xml = emitter.into_inner().into_inner();
    DavProp {
        name: elem.name.clone(),
        prefix: elem.prefix.clone(),
        namespace: elem.namespace.clone(),
        xml: Some(xml),
    }
}
