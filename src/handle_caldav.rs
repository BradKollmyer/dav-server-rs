use futures_util::StreamExt;
use headers::HeaderMapExt;
use http::{Request, Response, StatusCode};
use std::io::Cursor;
use xml::reader::{EventReader, XmlEvent};
use xmltree::{Element, XMLNode};

use crate::body::Body;
use crate::conditional::*;
use crate::errors::*;
use crate::fs::*;
use crate::xmltree_ext::*;
use crate::{DavInner, DavResult};

use crate::async_stream::AsyncStream;
use crate::caldav::*;
use crate::davheaders;
use crate::davpath::DavPath;
use crate::handle_props::PropWriter;

impl<C: Clone + Send + Sync + 'static> DavInner<C> {
    /// Handle REPORT method for CalDAV and CardDAV
    ///
    /// This method detects the namespace of the request body and routes
    /// to the appropriate CalDAV or CardDAV handler.
    pub(crate) async fn handle_report(
        &self,
        req: &Request<()>,
        body: &[u8],
    ) -> DavResult<Response<Body>> {
        // First, check if this is a CardDAV request by looking for CardDAV elements
        #[cfg(feature = "carddav")]
        if self.is_carddav_report(body) {
            return self.handle_carddav_report(req, body).await;
        }

        let path = self.path(req);
        self.ensure_visible(&path).await?;

        // Parse the REPORT request body as CalDAV
        let report_type = self.parse_report_request(body)?;

        // RFC 4791 7.8-7.10: these reports are scoped to calendar collections,
        // and supported-report-set only advertises them there. calendar-query
        // and calendar-multiget may also be addressed to a calendar object
        // resource inside a calendar; free-busy-query may not.
        let meta = self.fs.metadata(&path, &self.credentials).await?;
        let target_is_calendar = if meta.is_dir() {
            self.collection_is_calendar(&path, meta.as_ref()).await
        } else if matches!(report_type, CalDavReportType::FreeBusyQuery { .. }) {
            false
        } else {
            let parent = path.parent();
            match self.fs.metadata(&parent, &self.credentials).await {
                Ok(pmeta) => self.collection_is_calendar(&parent, pmeta.as_ref()).await,
                Err(_) => false,
            }
        };
        if !target_is_calendar {
            return Err(DavError::Status(StatusCode::FORBIDDEN));
        }

        match report_type {
            CalDavReportType::CalendarQuery(query) => {
                self.handle_calendar_query(&path, query).await
            }
            CalDavReportType::CalendarMultiget { hrefs } => {
                self.handle_calendar_multiget(&path, hrefs).await
            }
            CalDavReportType::FreeBusyQuery { time_range } => {
                self.handle_freebusy_query(&path, time_range).await
            }
        }
    }

    /// Check if the REPORT request body is a CardDAV request
    ///
    /// This parses the XML root element to check if it's a CardDAV request
    /// by examining the namespace and element name.
    #[cfg(feature = "carddav")]
    fn is_carddav_report(&self, body: &[u8]) -> bool {
        use crate::carddav::NS_CARDDAV_URI;

        if body.is_empty() {
            return false;
        }

        // Parse just enough to get the root element's name and namespace
        let cursor = Cursor::new(body);
        let parser = EventReader::new(cursor);

        for event in parser {
            match event {
                Ok(XmlEvent::StartElement {
                    name, namespace, ..
                }) => {
                    // Check if this is a CardDAV element by namespace
                    if let Some(prefix) = &name.prefix
                        && let Some(uri) = namespace.get(prefix)
                        && uri == NS_CARDDAV_URI
                    {
                        return true;
                    }

                    // Also check by element name for common CardDAV REPORT types
                    match name.local_name.as_str() {
                        "addressbook-query" | "addressbook-multiget" => return true,
                        _ => return false,
                    }
                }
                Err(_) => return false,
                _ => continue,
            }
        }

        false
    }

    /// Handle CalDAV MKCALENDAR method
    pub(crate) async fn handle_mkcalendar(
        &self,
        req: &Request<()>,
        body: &[u8],
    ) -> DavResult<Response<Body>> {
        let path = self.path(req);
        self.ensure_visible(&path).await?;
        let meta = self.fs.metadata(&path, &self.credentials).await;

        let res = if_match_get_tokens(
            req,
            meta.as_ref().map(|v| v.as_ref()).ok(),
            self.fs.as_ref(),
            &self.ls,
            &path,
            &self.credentials,
        )
        .await;
        let tokens = match res {
            Ok(t) => t,
            Err(s) => return Err(DavError::Status(s)),
        };

        if let Some(ref locksystem) = self.ls {
            let principal = self.principal.as_deref();
            if let Err(_l) = locksystem
                .check(&path, principal, false, false, &tokens)
                .await
            {
                return Err(DavError::Status(StatusCode::LOCKED));
            }
        }

        let mkcalendar = if body.is_empty() {
            None
        } else {
            let tree = Element::parse2(Cursor::new(body))?;
            if tree.name != "mkcalendar" || tree.namespace.as_deref() != Some(NS_CALDAV_URI) {
                return Err(DavError::StatusClose(StatusCode::BAD_REQUEST));
            }
            Some(tree)
        };

        let parent = path.parent();
        if let Ok(meta) = self.fs.metadata(&parent, &self.credentials).await
            && self.collection_is_calendar(&parent, meta.as_ref()).await
        {
            return Err(DavError::Status(StatusCode::FORBIDDEN));
        }

        match self.fs.create_dir(&path, &self.credentials).await {
            Err(FsError::Exists) => return Err(DavError::Status(StatusCode::METHOD_NOT_ALLOWED)),
            Err(FsError::NotFound) => return Err(DavError::Status(StatusCode::CONFLICT)),
            Err(e) => return Err(DavError::FsError(e)),
            Ok(()) => {}
        }

        self.fs.mark_calendar(&path, &self.credentials).await?;

        if let Some(tree) = mkcalendar {
            #[cfg(feature = "proppatch")]
            self.apply_set_props(&path, &tree).await?;
            #[cfg(not(feature = "proppatch"))]
            let _ = tree;
        }

        let mut resp = Response::new(Body::empty());
        *resp.status_mut() = StatusCode::CREATED;
        resp.headers_mut().typed_insert(headers::ContentLength(0));

        Ok(resp)
    }

    fn parse_report_request(&self, body: &[u8]) -> DavResult<CalDavReportType> {
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
                "calendar-query" => {
                    let query = self.parse_calendar_query(root)?;
                    Ok(CalDavReportType::CalendarQuery(query))
                }
                "calendar-multiget" => {
                    let hrefs = self.parse_calendar_multiget(root)?;
                    Ok(CalDavReportType::CalendarMultiget { hrefs })
                }
                "free-busy-query" => {
                    let time_range = self.parse_freebusy_query(root)?;
                    Ok(CalDavReportType::FreeBusyQuery { time_range })
                }
                _ => Err(DavError::StatusClose(StatusCode::BAD_REQUEST)),
            }
        } else {
            Err(DavError::StatusClose(StatusCode::BAD_REQUEST))
        }
    }

    fn parse_calendar_query(&self, root: &Element) -> DavResult<CalendarQuery> {
        let mut query = CalendarQuery {
            comp_filter: None,
            time_range: None,
            properties: Vec::new(),
        };

        for child in &root.children {
            if let XMLNode::Element(elem) = child {
                match elem.name.as_str() {
                    "filter" => {
                        for filter_child in &elem.children {
                            if let XMLNode::Element(filter_elem) = filter_child
                                && filter_elem.name == "comp-filter"
                            {
                                query.comp_filter = Some(self.parse_component_filter(filter_elem)?);
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
                    _ => {}
                }
            }
        }

        // RFC 4791 7.8.1: CALDAV:filter (with a nested comp-filter) is required.
        if query.comp_filter.is_none() {
            return Err(DavError::StatusClose(StatusCode::BAD_REQUEST));
        }

        Ok(query)
    }

    fn parse_component_filter(&self, elem: &Element) -> DavResult<ComponentFilter> {
        let name = elem
            .attributes
            .get("name")
            .ok_or(DavError::StatusClose(StatusCode::BAD_REQUEST))?
            .clone();

        let mut filter = ComponentFilter {
            name,
            is_not_defined: false,
            time_range: None,
            prop_filters: Vec::new(),
            comp_filters: Vec::new(),
        };

        for child in &elem.children {
            if let XMLNode::Element(child_elem) = child {
                match child_elem.name.as_str() {
                    "is-not-defined" => {
                        filter.is_not_defined = true;
                    }
                    "time-range" => {
                        filter.time_range = Some(self.parse_time_range(child_elem)?);
                    }
                    "prop-filter" => {
                        filter
                            .prop_filters
                            .push(self.parse_property_filter(child_elem)?);
                    }
                    "comp-filter" => {
                        filter
                            .comp_filters
                            .push(self.parse_component_filter(child_elem)?);
                    }
                    _ => {}
                }
            }
        }

        Ok(filter)
    }

    fn parse_property_filter(&self, elem: &Element) -> DavResult<PropertyFilter> {
        let name = elem
            .attributes
            .get("name")
            .ok_or(DavError::StatusClose(StatusCode::BAD_REQUEST))?
            .clone();

        let mut filter = PropertyFilter {
            name,
            is_not_defined: false,
            text_match: None,
            time_range: None,
            param_filters: Vec::new(),
        };

        for child in &elem.children {
            if let XMLNode::Element(child_elem) = child {
                match child_elem.name.as_str() {
                    "is-not-defined" => {
                        filter.is_not_defined = true;
                    }
                    "time-range" => {
                        filter.time_range = Some(self.parse_time_range(child_elem)?);
                    }
                    "text-match" => {
                        filter.text_match = Some(self.parse_text_match(child_elem)?);
                    }
                    "param-filter" => {
                        filter
                            .param_filters
                            .push(self.parse_param_filter(child_elem)?);
                    }
                    _ => {}
                }
            }
        }

        Ok(filter)
    }

    fn parse_param_filter(&self, elem: &Element) -> DavResult<ParameterFilter> {
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
                        filter.text_match = Some(self.parse_text_match(child_elem)?);
                    }
                    _ => {}
                }
            }
        }

        Ok(filter)
    }

    fn parse_time_range(&self, elem: &Element) -> DavResult<TimeRange> {
        Ok(TimeRange {
            start: elem.attributes.get("start").cloned(),
            end: elem.attributes.get("end").cloned(),
        })
    }

    fn parse_text_match(&self, elem: &Element) -> DavResult<TextMatch> {
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

    fn parse_calendar_multiget(&self, root: &Element) -> DavResult<Vec<String>> {
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

    fn parse_freebusy_query(&self, root: &Element) -> DavResult<TimeRange> {
        for child in &root.children {
            if let XMLNode::Element(elem) = child
                && elem.name == "time-range"
            {
                return self.parse_time_range(elem);
            }
        }

        Err(DavError::StatusClose(StatusCode::BAD_REQUEST))
    }

    async fn handle_calendar_query(
        &self,
        path: &DavPath,
        query: CalendarQuery,
    ) -> DavResult<Response<Body>> {
        // Get directory listing
        let stream = self
            .fs
            .read_dir(path, self.get_read_dir_meta(), &self.credentials)
            .await?;
        let mut results = Vec::new();

        let items: Vec<_> = stream.collect().await;
        for item in items {
            match item {
                Ok(dirent) => {
                    if self.hidden_in_listing(dirent.as_ref()).await {
                        continue;
                    }
                    let mut item_path = path.clone();
                    item_path.push_segment(&dirent.name());

                    // Check if this is a calendar resource, and append content to result
                    if let Ok(mut file) = self
                        .fs
                        .open(&item_path, OpenOptions::read(), &self.credentials)
                        .await
                        && let Ok(metadata) = file.metadata().await
                        && metadata.len() <= DEFAULT_MAX_RESOURCE_SIZE
                        && let Ok(data) =
                            read_file_to_end(file.as_mut(), metadata.len() as usize).await
                        && is_calendar_data(&data)
                    {
                        let content = String::from_utf8_lossy(&data);

                        if self.matches_query(&content, &query) {
                            let etag = metadata.etag().unwrap_or_default().to_string();
                            results.push((item_path.clone(), etag, content.to_string()));
                            continue;
                        }
                    }
                }
                Err(_) => continue,
            }
        }

        // Generate multistatus response
        self.generate_calendar_multiget_response(results, Vec::new())
            .await
    }

    async fn handle_calendar_multiget(
        &self,
        path: &DavPath,
        hrefs: Vec<String>,
    ) -> DavResult<Response<Body>> {
        let mut results = Vec::new();
        let mut missing_hrefs: Vec<String> = Vec::new();

        for href in &hrefs {
            // RFC 4791 7.9: hrefs must be calendar object resources in this
            // collection, or the request URI itself when the REPORT is
            // addressed to a calendar object resource.
            if let Ok(item_path) = DavPath::from_str_and_prefix(href, &self.prefix)
                && (item_path.is_in_collection(path) || item_path == *path)
                && self.ensure_visible(&item_path).await.is_ok()
                && let Ok(mut file) = self
                    .fs
                    .open(&item_path, OpenOptions::read(), &self.credentials)
                    .await
                && let Ok(metadata) = file.metadata().await
                && metadata.len() <= DEFAULT_MAX_RESOURCE_SIZE
                && let Ok(data) = read_file_to_end(file.as_mut(), metadata.len() as usize).await
                && is_calendar_data(&data)
            {
                let etag = metadata.etag().unwrap_or_default().to_string();
                let content = String::from_utf8_lossy(&data);
                results.push((item_path, etag, content.to_string()));
                continue;
            }

            missing_hrefs.push(href.clone());
        }

        self.generate_calendar_multiget_response(results, missing_hrefs)
            .await
    }

    async fn handle_freebusy_query(
        &self,
        path: &DavPath,
        time_range: TimeRange,
    ) -> DavResult<Response<Body>> {
        let Some((range_start, range_end)) = parse_freebusy_bounds(&time_range) else {
            return Err(DavError::StatusClose(StatusCode::BAD_REQUEST));
        };

        let stream = self
            .fs
            .read_dir(path, self.get_read_dir_meta(), &self.credentials)
            .await?;
        let mut busy = Vec::new();

        let items: Vec<_> = stream.collect().await;
        for item in items {
            let Ok(dirent) = item else { continue };
            if self.hidden_in_listing(dirent.as_ref()).await {
                continue;
            }
            let mut item_path = path.clone();
            item_path.push_segment(&dirent.name());

            if let Ok(mut file) = self
                .fs
                .open(&item_path, OpenOptions::read(), &self.credentials)
                .await
                && let Ok(metadata) = file.metadata().await
                && metadata.len() <= DEFAULT_MAX_RESOURCE_SIZE
                && let Ok(data) = read_file_to_end(file.as_mut(), metadata.len() as usize).await
                && is_calendar_data(&data)
            {
                let content = String::from_utf8_lossy(&data);
                if let Ok(calendar) = validate_calendar_data(&content) {
                    busy.extend(calendar_busy_intervals(&calendar, range_start, range_end));
                }
            }
        }

        let busy = merge_busy_intervals(busy);
        let ics = freebusy_calendar(range_start, range_end, &busy);
        let ics_len = ics.len() as u64;

        let mut resp = Response::new(Body::from(ics));
        *resp.status_mut() = StatusCode::OK;
        resp.headers_mut()
            .typed_insert(davheaders::ContentType("text/calendar".to_owned()));
        resp.headers_mut()
            .typed_insert(headers::ContentLength(ics_len));
        Ok(resp)
    }

    fn matches_query(&self, content: &str, query: &CalendarQuery) -> bool {
        calendar_matches_query(content, query)
    }

    #[cfg(feature = "caldav")]
    async fn generate_calendar_multiget_response(
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

            for (href, etag, calendar_data) in results {
                pw.write_calendar_data_response(&href, &etag, &calendar_data)?;
            }

            for missing_href in missing_hrefs {
                pw.write_calendar_not_found_response(&missing_href)?;
            }

            pw.close().await?;

            Ok(())
        }));

        Ok(resp)
    }
}
