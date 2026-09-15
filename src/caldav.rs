//! CalDAV (Calendaring Extensions to WebDAV) support
//!
//! This module provides CalDAV functionality on top of the base WebDAV implementation.
//! CalDAV is defined in RFC 4791 and provides standardized access to calendar data
//! using the iCalendar format.

#[cfg(feature = "caldav")]
use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, TimeDelta, Utc};
#[cfg(feature = "caldav")]
use icalendar::{Calendar, CalendarComponent, CalendarDateTime, Component, DatePerhapsTime};
use xmltree::Element;

use crate::davpath::DavPath;

// Re-export shared filter types
pub use crate::dav_filters::{ParameterFilter, TextMatch};

// CalDAV XML namespaces
pub const NS_CALDAV_URI: &str = "urn:ietf:params:xml:ns:caldav";
pub const NS_CALENDARSERVER_URI: &str = "http://calendarserver.org/ns/";

// CalDAV property names
pub const CALDAV_PROPERTIES: &[&str] = &[
    "C:calendar-description",
    "C:calendar-timezone",
    "C:supported-calendar-component-set",
    "C:supported-calendar-data",
    "C:max-resource-size",
    "C:min-date-time",
    "C:max-date-time",
    "C:max-instances",
    "C:max-attendees-per-instance",
    "C:calendar-home-set",
    "C:calendar-user-address-set",
    "C:schedule-inbox-URL",
    "C:schedule-outbox-URL",
];

/// The default caldav directory, which is being used for the preprovided filesystems. Path is without trailing slash
pub const DEFAULT_CALDAV_NAME: &str = "calendars";
pub const DEFAULT_CALDAV_DIRECTORY: &str = "/calendars";
pub const DEFAULT_CALDAV_DIRECTORY_ENDSLASH: &str = "/calendars/";

/// CalDAV resource types
#[derive(Debug, Clone, PartialEq)]
pub enum CalDavResourceType {
    Calendar,
    ScheduleInbox,
    ScheduleOutbox,
    CalendarObject,
    Regular,
}

/// CalDAV component types supported in a calendar collection
#[derive(Debug, Clone, PartialEq)]
pub enum CalendarComponentType {
    VEvent,
    VTodo,
    VJournal,
    VFreeBusy,
    VTimezone,
    VAlarm,
}

impl CalendarComponentType {
    pub fn as_str(&self) -> &'static str {
        match self {
            CalendarComponentType::VEvent => "VEVENT",
            CalendarComponentType::VTodo => "VTODO",
            CalendarComponentType::VJournal => "VJOURNAL",
            CalendarComponentType::VFreeBusy => "VFREEBUSY",
            CalendarComponentType::VTimezone => "VTIMEZONE",
            CalendarComponentType::VAlarm => "VALARM",
        }
    }
}

/// CalDAV calendar collection properties
#[derive(Debug, Clone)]
pub struct CalendarProperties {
    pub description: Option<String>,
    pub timezone: Option<String>,
    pub supported_components: Vec<CalendarComponentType>,
    pub max_resource_size: Option<u64>,
    pub color: Option<String>,
    pub display_name: Option<String>,
}

impl Default for CalendarProperties {
    fn default() -> Self {
        Self {
            description: None,
            timezone: None,
            supported_components: vec![
                CalendarComponentType::VEvent,
                CalendarComponentType::VTodo,
                CalendarComponentType::VJournal,
                CalendarComponentType::VFreeBusy,
            ],
            max_resource_size: Some(1024 * 1024), // 1MB default
            color: None,
            display_name: None,
        }
    }
}

/// Calendar query filters for REPORT requests
#[derive(Debug, Clone)]
pub struct CalendarQuery {
    pub comp_filter: Option<ComponentFilter>,
    pub time_range: Option<TimeRange>,
    pub properties: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ComponentFilter {
    pub name: String,
    pub is_not_defined: bool,
    pub time_range: Option<TimeRange>,
    pub prop_filters: Vec<PropertyFilter>,
    pub comp_filters: Vec<ComponentFilter>,
}

/// CalDAV property filter with time-range support
///
/// Note: CalDAV property filters include time-range which is not present
/// in the shared ParameterFilter. CardDAV has a similar struct without time_range.
#[derive(Debug, Clone)]
pub struct PropertyFilter {
    pub name: String,
    pub is_not_defined: bool,
    pub text_match: Option<TextMatch>,
    pub time_range: Option<TimeRange>,
    pub param_filters: Vec<ParameterFilter>,
}

#[derive(Debug, Clone)]
pub struct TimeRange {
    /// ISO 8601 format
    pub start: Option<String>,
    /// ISO 8601 format
    pub end: Option<String>,
}

/// CalDAV REPORT request types
#[derive(Debug, Clone)]
pub enum CalDavReportType {
    CalendarQuery(CalendarQuery),
    CalendarMultiget { hrefs: Vec<String> },
    FreeBusyQuery { time_range: TimeRange },
}

/// Helper functions for CalDAV XML generation
pub fn create_supported_calendar_component_set(components: &[CalendarComponentType]) -> Element {
    let mut elem = Element::new("C:supported-calendar-component-set");
    elem.namespace = Some(NS_CALDAV_URI.to_string());

    for comp in components {
        let mut comp_elem = Element::new("C:comp");
        comp_elem.namespace = Some(NS_CALDAV_URI.to_string());
        comp_elem
            .attributes
            .insert("name".to_string(), comp.as_str().to_string());
        elem.children.push(xmltree::XMLNode::Element(comp_elem));
    }

    elem
}

pub fn create_supported_calendar_data() -> Element {
    let mut elem = Element::new("C:supported-calendar-data");
    elem.namespace = Some(NS_CALDAV_URI.to_string());

    let mut calendar_data = Element::new("C:calendar-data");
    calendar_data.namespace = Some(NS_CALDAV_URI.to_string());
    calendar_data
        .attributes
        .insert("content-type".to_string(), "text/calendar".to_string());
    calendar_data
        .attributes
        .insert("version".to_string(), "2.0".to_string());

    elem.children.push(xmltree::XMLNode::Element(calendar_data));
    elem
}

pub fn create_calendar_home_set(prefix: &str, path: &str) -> Element {
    let mut elem = Element::new("C:calendar-home-set");
    elem.namespace = Some(NS_CALDAV_URI.to_string());

    let mut href = Element::new("D:href");
    href.namespace = Some("DAV:".to_string());
    href.children
        .push(xmltree::XMLNode::Text(format!("{prefix}{path}")));

    elem.children.push(xmltree::XMLNode::Element(href));
    elem
}

/// Check if a path is within the default CalDAV directory. Expects path without prefix.
pub(crate) fn is_path_in_caldav_directory(dav_path: &DavPath) -> bool {
    let path_string = dav_path.to_string();
    path_string.len() > DEFAULT_CALDAV_DIRECTORY_ENDSLASH.len()
        && path_string.starts_with(DEFAULT_CALDAV_DIRECTORY_ENDSLASH)
}

/// Check if a resource is a calendar collection based on resource type
pub fn is_calendar_collection(resource_type: &[Element]) -> bool {
    resource_type
        .iter()
        .any(|elem| elem.name == "calendar" && elem.namespace.as_deref() == Some(NS_CALDAV_URI))
}

/// Check if content appears to be iCalendar data
pub fn is_calendar_data(content: &[u8]) -> bool {
    if !content.starts_with(b"BEGIN:VCALENDAR") {
        return false;
    }

    let trimmed = content.trim_ascii_end();
    trimmed.ends_with(b"END:VCALENDAR")
}

/// Validate iCalendar data using the icalendar crate
///
/// This function validates that the content is a well-formed iCalendar object.
/// Use this function in your application layer to validate calendar data
/// before or after writing to the filesystem.
///
/// # Example
///
/// ```ignore
/// use dav_server::caldav::validate_calendar_data;
///
/// let ical = "BEGIN:VCALENDAR\nVERSION:2.0\nBEGIN:VEVENT\n...";
/// match validate_calendar_data(ical) {
///     Ok(_) => println!("Valid iCalendar"),
///     Err(e) => println!("Invalid iCalendar: {}", e),
/// }
/// ```
#[cfg(feature = "caldav")]
pub fn validate_calendar_data(content: &str) -> Result<Calendar, String> {
    content
        .parse::<Calendar>()
        .map_err(|e| format!("Invalid iCalendar data: {}", e))
}

/// Evaluate a `calendar-query` filter against iCalendar text.
///
/// Unparseable data does not match (the REPORT skips that resource).
#[cfg(feature = "caldav")]
pub(crate) fn calendar_matches_query(content: &str, query: &CalendarQuery) -> bool {
    let Some(filter) = query.comp_filter.as_ref() else {
        return false;
    };
    let Ok(calendar) = validate_calendar_data(content) else {
        return false;
    };
    matches_calendar(&calendar, filter)
}

#[cfg(feature = "caldav")]
fn matches_calendar(calendar: &Calendar, filter: &ComponentFilter) -> bool {
    if filter.name.eq_ignore_ascii_case("VCALENDAR") {
        if filter.is_not_defined {
            return false;
        }
        // time-range on VCALENDAR is not defined by RFC 4791 9.9; treat it as
        // "any child component overlaps" so a misplaced range is not a no-op.
        if let Some(tr) = &filter.time_range
            && !calendar
                .components
                .iter()
                .any(|c| calendar_component_overlaps_time_range(c, tr))
        {
            return false;
        }
        if !filter
            .prop_filters
            .iter()
            .all(|pf| calendar_matches_prop_filter(calendar, pf))
        {
            return false;
        }
        calendar_nested_filters_match(calendar, &filter.comp_filters)
    } else {
        // RFC 4791 requires a top-level VCALENDAR filter; still evaluate a
        // VEVENT/VTODO root against children so those queries are not no-ops.
        calendar_nested_filters_match(calendar, std::slice::from_ref(filter))
    }
}

#[cfg(feature = "caldav")]
fn calendar_nested_filters_match(calendar: &Calendar, filters: &[ComponentFilter]) -> bool {
    filters.iter().all(|cf| {
        let children: Vec<&CalendarComponent> = calendar
            .components
            .iter()
            .filter(|c| calendar_component_name_matches(c, &cf.name))
            .collect();
        if cf.is_not_defined {
            children.is_empty()
        } else if children.is_empty() {
            false
        } else {
            children
                .iter()
                .any(|c| calendar_component_matches_filter(c, cf))
        }
    })
}

#[cfg(feature = "caldav")]
fn calendar_component_name_matches(comp: &CalendarComponent, name: &str) -> bool {
    match comp {
        CalendarComponent::Event(_) => name.eq_ignore_ascii_case("VEVENT"),
        CalendarComponent::Todo(_) => name.eq_ignore_ascii_case("VTODO"),
        CalendarComponent::Venue(_) => name.eq_ignore_ascii_case("VVENUE"),
        CalendarComponent::Other(other) => other.component_kind().eq_ignore_ascii_case(name),
        _ => false,
    }
}

#[cfg(feature = "caldav")]
fn calendar_component_matches_filter(comp: &CalendarComponent, filter: &ComponentFilter) -> bool {
    match comp {
        CalendarComponent::Event(c) => component_matches_filter(c, filter),
        CalendarComponent::Todo(c) => component_matches_filter(c, filter),
        CalendarComponent::Venue(c) => component_matches_filter(c, filter),
        CalendarComponent::Other(c) => component_matches_filter(c, filter),
        _ => false,
    }
}

#[cfg(feature = "caldav")]
fn calendar_component_overlaps_time_range(comp: &CalendarComponent, tr: &TimeRange) -> bool {
    match comp {
        CalendarComponent::Event(c) => component_overlaps_time_range(c, tr),
        CalendarComponent::Todo(c) => component_overlaps_time_range(c, tr),
        CalendarComponent::Venue(c) => component_overlaps_time_range(c, tr),
        CalendarComponent::Other(c) => component_overlaps_time_range(c, tr),
        _ => false,
    }
}

#[cfg(feature = "caldav")]
fn component_matches_filter<C: Component>(comp: &C, filter: &ComponentFilter) -> bool {
    if let Some(tr) = &filter.time_range
        && !component_overlaps_time_range(comp, tr)
    {
        return false;
    }
    if !filter
        .prop_filters
        .iter()
        .all(|pf| component_matches_prop_filter(comp, pf))
    {
        return false;
    }
    nested_other_filters_match(comp, &filter.comp_filters)
}

#[cfg(feature = "caldav")]
fn nested_other_filters_match<C: Component>(comp: &C, filters: &[ComponentFilter]) -> bool {
    filters.iter().all(|cf| {
        let children: Vec<_> = comp
            .components()
            .iter()
            .filter(|c| c.component_kind().eq_ignore_ascii_case(&cf.name))
            .collect();
        if cf.is_not_defined {
            children.is_empty()
        } else if children.is_empty() {
            false
        } else {
            children.iter().any(|c| component_matches_filter(*c, cf))
        }
    })
}

#[cfg(feature = "caldav")]
fn component_overlaps_time_range<C: Component>(comp: &C, tr: &TimeRange) -> bool {
    let Some(range) = parse_time_range_bounds(tr) else {
        return false;
    };
    let Some((comp_start, comp_end)) = component_span(comp) else {
        return false;
    };
    time_spans_overlap(comp_start, comp_end, range.start, range.end)
}

#[cfg(feature = "caldav")]
struct ParsedTimeRange {
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
}

/// Parse CALDAV:time-range start/end (RFC 5545 DATE or UTC DATE-TIME).
#[cfg(feature = "caldav")]
fn parse_time_range_bounds(tr: &TimeRange) -> Option<ParsedTimeRange> {
    let start = match tr.start.as_deref() {
        Some(s) => Some(parse_caldav_date_time(s)?),
        None => None,
    };
    let end = match tr.end.as_deref() {
        Some(s) => Some(parse_caldav_date_time(s)?),
        None => None,
    };
    if start.is_none() && end.is_none() {
        None
    } else {
        Some(ParsedTimeRange { start, end })
    }
}

#[cfg(feature = "caldav")]
fn parse_caldav_date_time(s: &str) -> Option<DateTime<Utc>> {
    let s = s.trim();
    if let Ok(ndt) = NaiveDateTime::parse_from_str(s, "%Y%m%dT%H%M%SZ") {
        return Some(ndt.and_utc());
    }
    if let Ok(ndt) = NaiveDateTime::parse_from_str(s, "%Y%m%dT%H%M%S") {
        return Some(ndt.and_utc());
    }
    if let Ok(d) = NaiveDate::parse_from_str(s, "%Y%m%d") {
        return Some(d.and_time(NaiveTime::MIN).and_utc());
    }
    None
}

#[cfg(feature = "caldav")]
fn to_utc(dt: &DatePerhapsTime) -> DateTime<Utc> {
    match dt {
        DatePerhapsTime::DateTime(CalendarDateTime::Utc(dt)) => *dt,
        DatePerhapsTime::DateTime(CalendarDateTime::Floating(ndt)) => ndt.and_utc(),
        DatePerhapsTime::DateTime(CalendarDateTime::WithTimezone { date_time, .. }) => {
            date_time.and_utc()
        }
        DatePerhapsTime::Date(date) => date.and_time(NaiveTime::MIN).and_utc(),
    }
}

/// Component interval from DTSTART/DTEND/DUE. DATE-only DTSTART without an end
/// lasts one day; DATE-TIME without an end is a zero-duration point.
#[cfg(feature = "caldav")]
fn component_span<C: Component>(comp: &C) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    let start = comp.get_start();
    let end = comp.get_end();
    let due = comp
        .properties()
        .get("DUE")
        .and_then(DatePerhapsTime::from_property);

    match (start.as_ref(), end.as_ref(), due.as_ref()) {
        (Some(s), Some(e), _) => Some((to_utc(s), to_utc(e))),
        (Some(s), None, Some(d)) => Some((to_utc(s), to_utc(d))),
        (Some(s), None, None) => {
            let start_utc = to_utc(s);
            let end_utc = match s {
                DatePerhapsTime::Date(_) => start_utc + TimeDelta::days(1),
                DatePerhapsTime::DateTime(_) => start_utc,
            };
            Some((start_utc, end_utc))
        }
        (None, None, Some(d)) => {
            let due_utc = to_utc(d);
            Some((due_utc, due_utc))
        }
        (None, Some(e), _) => {
            let end_utc = to_utc(e);
            Some((end_utc, end_utc))
        }
        _ => None,
    }
}

/// Overlap of `[comp_start, comp_end)` with `[range_start, range_end)`.
/// Zero-duration components use RFC 4791's closed-open point test.
#[cfg(feature = "caldav")]
fn time_spans_overlap(
    comp_start: DateTime<Utc>,
    comp_end: DateTime<Utc>,
    range_start: Option<DateTime<Utc>>,
    range_end: Option<DateTime<Utc>>,
) -> bool {
    let before_end = range_end.is_none_or(|end| comp_start < end);
    let after_start = if comp_start == comp_end {
        range_start.is_none_or(|start| start <= comp_start)
    } else {
        range_start.is_none_or(|start| start < comp_end)
    };
    before_end && after_start
}

#[cfg(feature = "caldav")]
fn calendar_matches_prop_filter(calendar: &Calendar, pf: &PropertyFilter) -> bool {
    let values: Vec<&str> = calendar
        .properties
        .iter()
        .filter(|p| p.key().eq_ignore_ascii_case(&pf.name))
        .map(|p| p.value())
        .collect();
    matches_prop_values(&values, pf)
}

#[cfg(feature = "caldav")]
fn component_matches_prop_filter<C: Component>(comp: &C, pf: &PropertyFilter) -> bool {
    let mut values: Vec<&str> = Vec::new();
    for (key, prop) in comp.properties() {
        if key.eq_ignore_ascii_case(&pf.name) {
            values.push(prop.value());
        }
    }
    for (key, props) in comp.multi_properties() {
        if key.eq_ignore_ascii_case(&pf.name) {
            values.extend(props.iter().map(|p| p.value()));
        }
    }
    matches_prop_values(&values, pf)
}

/// Property exists (or is-not-defined), plus optional substring text-match on
/// the property value. param-filter and prop-filter time-range are ignored.
#[cfg(feature = "caldav")]
fn matches_prop_values(values: &[&str], pf: &PropertyFilter) -> bool {
    if pf.is_not_defined {
        return values.is_empty();
    }
    if values.is_empty() {
        return false;
    }
    match &pf.text_match {
        Some(tm) => values.iter().any(|v| text_matches_value(v, tm)),
        None => true,
    }
}

#[cfg(feature = "caldav")]
fn text_matches_value(value: &str, tm: &TextMatch) -> bool {
    let case_insensitive = tm.collation.as_deref().is_none_or(|c| {
        c.eq_ignore_ascii_case("i;ascii-casemap") || c.eq_ignore_ascii_case("i;unicode-casemap")
    });
    let (haystack, needle) = if case_insensitive {
        (value.to_lowercase(), tm.text.to_lowercase())
    } else {
        (value.to_string(), tm.text.clone())
    };
    let matched = match tm.match_type.as_deref() {
        Some("equals") => haystack == needle,
        Some("starts-with") => haystack.starts_with(&needle),
        Some("ends-with") => haystack.ends_with(&needle),
        _ => haystack.contains(&needle),
    };
    if tm.negate_condition {
        !matched
    } else {
        matched
    }
}

#[cfg(all(test, feature = "caldav"))]
mod tests {
    use super::*;

    fn vcalendar_with(child: ComponentFilter) -> CalendarQuery {
        CalendarQuery {
            comp_filter: Some(ComponentFilter {
                name: "VCALENDAR".into(),
                is_not_defined: false,
                time_range: None,
                prop_filters: Vec::new(),
                comp_filters: vec![child],
            }),
            time_range: None,
            properties: Vec::new(),
        }
    }

    fn vevent_filter() -> ComponentFilter {
        ComponentFilter {
            name: "VEVENT".into(),
            is_not_defined: false,
            time_range: None,
            prop_filters: Vec::new(),
            comp_filters: Vec::new(),
        }
    }

    #[test]
    fn unparseable_ics_does_not_match() {
        let query = vcalendar_with(vevent_filter());
        assert!(!calendar_matches_query(
            "BEGIN:VCALENDAR\nNOT VALID\nEND:VCALENDAR",
            &query
        ));
    }
}

/// Extract the UID from calendar data
///
/// Handles both standard `UID:value` and properties with parameters.
pub fn extract_calendar_uid(content: &str) -> Option<String> {
    for line in content.lines() {
        let line = line.trim();
        // Handle simple UID:VALUE
        if let Some(value) = line.strip_prefix("UID:") {
            return Some(value.to_string());
        }
        // Handle UID with parameters: UID;PARAMS:VALUE
        if let Some(rest) = line.strip_prefix("UID;")
            && let Some(colon_pos) = rest.find(':')
        {
            return Some(rest[colon_pos + 1..].to_string());
        }
    }
    None
}

/// Generate a simple calendar collection resource type XML
pub fn calendar_resource_type() -> Vec<Element> {
    let mut collection = Element::new("D:collection");
    collection.namespace = Some("DAV:".to_string());

    let mut calendar = Element::new("C:calendar");
    calendar.namespace = Some(NS_CALDAV_URI.to_string());

    vec![collection, calendar]
}

/// Generate schedule inbox resource type XML
pub fn schedule_inbox_resource_type() -> Vec<Element> {
    let mut collection = Element::new("D:collection");
    collection.namespace = Some("DAV:".to_string());

    let mut schedule_inbox = Element::new("C:schedule-inbox");
    schedule_inbox.namespace = Some(NS_CALDAV_URI.to_string());

    vec![collection, schedule_inbox]
}

/// Generate schedule outbox resource type XML
pub fn schedule_outbox_resource_type() -> Vec<Element> {
    let mut collection = Element::new("D:collection");
    collection.namespace = Some("DAV:".to_string());

    let mut schedule_outbox = Element::new("C:schedule-outbox");
    schedule_outbox.namespace = Some(NS_CALDAV_URI.to_string());

    vec![collection, schedule_outbox]
}
