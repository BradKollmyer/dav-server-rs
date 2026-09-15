//! CalDAV (Calendaring Extensions to WebDAV) support
//!
//! This module provides CalDAV functionality on top of the base WebDAV implementation.
//! CalDAV is defined in RFC 4791 and provides standardized access to calendar data
//! using the iCalendar format.

#[cfg(feature = "caldav")]
use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, TimeDelta, Utc};
#[cfg(feature = "caldav")]
use icalendar::{
    Calendar, CalendarComponent, CalendarDateTime, Component, DatePerhapsTime, Property,
};
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

/// Default maximum calendar object size (1MB), advertised as `C:max-resource-size`.
pub const DEFAULT_MAX_RESOURCE_SIZE: u64 = 1024 * 1024;

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
            max_resource_size: Some(DEFAULT_MAX_RESOURCE_SIZE),
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

/// CalDAV property filter.
///
/// Matching uses `name`, `is_not_defined`, optional `text-match`,
/// `time_range` on DATE/DATE-TIME values, and `param_filters`.
/// CardDAV has a similar struct without `time_range`.
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

/// Immediate child of `/calendars` (`/calendars/<name>` or `/calendars/<name>/`).
/// The home itself and nested paths are not calendar collections (RFC 4791 4.2).
/// Compares prefix-stripped URL bytes, not PathBuf display.
/// Used for calendar-home-set URLs; collection type is stored on metadata.
#[allow(dead_code)]
pub(crate) fn is_path_in_caldav_directory(dav_path: &DavPath) -> bool {
    is_immediate_child_of(
        dav_path.as_bytes(),
        DEFAULT_CALDAV_DIRECTORY_ENDSLASH.as_bytes(),
    )
}

fn is_immediate_child_of(path: &[u8], parent_slash: &[u8]) -> bool {
    let path = match path.strip_suffix(b"/") {
        Some(rest) if !rest.is_empty() => rest,
        _ => path,
    };
    path.strip_prefix(parent_slash)
        .is_some_and(|name| !name.is_empty() && !name.contains(&b'/'))
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
/// PUT or PATCH into a calendar collection applies this check and returns 403
/// on failure. Applications may also call it directly.
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
pub(crate) fn parse_caldav_date_time(s: &str) -> Option<DateTime<Utc>> {
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

/// Resolve a DTSTART/DTEND/DUE value to UTC. A TZID naming an Olson zone is
/// converted through chrono-tz; a TZID that is not an Olson name (a custom
/// VTIMEZONE id) or a local time that does not exist or is ambiguous at a DST
/// transition falls back to reading the wall-clock time as UTC.
#[cfg(feature = "caldav")]
fn to_utc(dt: &DatePerhapsTime) -> DateTime<Utc> {
    match dt {
        DatePerhapsTime::DateTime(CalendarDateTime::Utc(dt)) => *dt,
        DatePerhapsTime::DateTime(CalendarDateTime::Floating(ndt)) => ndt.and_utc(),
        DatePerhapsTime::DateTime(cdt @ CalendarDateTime::WithTimezone { date_time, .. }) => {
            cdt.try_into_utc().unwrap_or_else(|| date_time.and_utc())
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

/// RFC 5545 UTC DATE-TIME (`YYYYMMDDTHHMMSSZ`) from a parsed instant.
#[cfg(feature = "caldav")]
pub(crate) fn format_caldav_utc(dt: DateTime<Utc>) -> String {
    dt.format("%Y%m%dT%H%M%SZ").to_string()
}

/// Both `time-range` start and end must parse as CalDAV DATE/DATE-TIME.
#[cfg(feature = "caldav")]
pub(crate) fn parse_freebusy_bounds(tr: &TimeRange) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    Some((
        parse_caldav_date_time(tr.start.as_deref()?)?,
        parse_caldav_date_time(tr.end.as_deref()?)?,
    ))
}

/// Busy intervals from overlapping opaque VEVENT and VFREEBUSY FREEBUSY periods.
/// Recurring VEVENT: only the base instance is included; RRULE is not expanded.
#[cfg(feature = "caldav")]
pub(crate) fn calendar_busy_intervals(
    calendar: &Calendar,
    range_start: DateTime<Utc>,
    range_end: DateTime<Utc>,
) -> Vec<(DateTime<Utc>, DateTime<Utc>)> {
    let mut busy = Vec::new();
    for comp in &calendar.components {
        match comp {
            CalendarComponent::Event(event) => {
                if component_is_transparent(event) {
                    continue;
                }
                if let Some((start, end)) = component_span(event)
                    && time_spans_overlap(start, end, Some(range_start), Some(range_end))
                {
                    busy.push((start, end));
                }
            }
            CalendarComponent::Other(other)
                if other.component_kind().eq_ignore_ascii_case("VFREEBUSY") =>
            {
                collect_vfreebusy_periods(other, range_start, range_end, &mut busy);
            }
            _ => {}
        }
    }
    busy
}

#[cfg(feature = "caldav")]
fn component_is_transparent<C: Component>(comp: &C) -> bool {
    for (key, prop) in comp.properties() {
        if key.eq_ignore_ascii_case("TRANSP") {
            return prop.value().eq_ignore_ascii_case("TRANSPARENT");
        }
    }
    false
}

#[cfg(feature = "caldav")]
fn collect_vfreebusy_periods<C: Component>(
    comp: &C,
    range_start: DateTime<Utc>,
    range_end: DateTime<Utc>,
    busy: &mut Vec<(DateTime<Utc>, DateTime<Utc>)>,
) {
    let mut props: Vec<&Property> = Vec::new();
    for (key, prop) in comp.properties() {
        if key.eq_ignore_ascii_case("FREEBUSY") {
            props.push(prop);
        }
    }
    for (key, multi) in comp.multi_properties() {
        if key.eq_ignore_ascii_case("FREEBUSY") {
            props.extend(multi.iter());
        }
    }
    for prop in props {
        if freebusy_fbtype_is_free(prop) {
            continue;
        }
        for period in prop.value().split(',') {
            if let Some((start, end)) = parse_freebusy_period(period)
                && time_spans_overlap(start, end, Some(range_start), Some(range_end))
            {
                busy.push((start, end));
            }
        }
    }
}

#[cfg(feature = "caldav")]
fn freebusy_fbtype_is_free(prop: &Property) -> bool {
    prop.params()
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("FBTYPE"))
        .is_some_and(|(_, p)| p.value().eq_ignore_ascii_case("FREE"))
}

#[cfg(feature = "caldav")]
fn parse_freebusy_period(period: &str) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    let (start, end) = period.split_once('/')?;
    Some((
        parse_caldav_date_time(start.trim())?,
        parse_caldav_date_time(end.trim())?,
    ))
}

#[cfg(feature = "caldav")]
pub(crate) fn merge_busy_intervals(
    mut intervals: Vec<(DateTime<Utc>, DateTime<Utc>)>,
) -> Vec<(DateTime<Utc>, DateTime<Utc>)> {
    if intervals.len() <= 1 {
        return intervals;
    }
    intervals.sort_by_key(|(start, _)| *start);
    let mut merged = Vec::with_capacity(intervals.len());
    let mut current = intervals[0];
    for next in intervals.into_iter().skip(1) {
        if next.0 <= current.1 {
            if next.1 > current.1 {
                current.1 = next.1;
            }
        } else {
            merged.push(current);
            current = next;
        }
    }
    merged.push(current);
    merged
}

/// RFC 4791 7.10 `text/calendar` VFREEBUSY document. Values are formatted from
/// parsed instants so request attributes are never copied into the ICS.
#[cfg(feature = "caldav")]
pub(crate) fn freebusy_calendar(
    range_start: DateTime<Utc>,
    range_end: DateTime<Utc>,
    busy: &[(DateTime<Utc>, DateTime<Utc>)],
) -> String {
    let mut ics = String::from(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//dav-server//CalDAV//EN\r\nBEGIN:VFREEBUSY\r\n",
    );
    ics.push_str("UID:");
    ics.push_str(&uuid::Uuid::new_v4().to_string());
    ics.push_str("\r\nDTSTAMP:");
    ics.push_str(&format_caldav_utc(Utc::now()));
    ics.push_str("\r\nDTSTART:");
    ics.push_str(&format_caldav_utc(range_start));
    ics.push_str("\r\nDTEND:");
    ics.push_str(&format_caldav_utc(range_end));
    ics.push_str("\r\n");
    for (start, end) in busy {
        ics.push_str("FREEBUSY:");
        ics.push_str(&format_caldav_utc(*start));
        ics.push('/');
        ics.push_str(&format_caldav_utc(*end));
        ics.push_str("\r\n");
    }
    ics.push_str("END:VFREEBUSY\r\nEND:VCALENDAR\r\n");
    ics
}

#[cfg(feature = "caldav")]
fn calendar_matches_prop_filter(calendar: &Calendar, pf: &PropertyFilter) -> bool {
    let props: Vec<&Property> = calendar
        .properties
        .iter()
        .filter(|p| p.key().eq_ignore_ascii_case(&pf.name))
        .collect();
    matches_prop_values(&props, pf)
}

#[cfg(feature = "caldav")]
fn component_matches_prop_filter<C: Component>(comp: &C, pf: &PropertyFilter) -> bool {
    let mut props: Vec<&Property> = Vec::new();
    for (key, prop) in comp.properties() {
        if key.eq_ignore_ascii_case(&pf.name) {
            props.push(prop);
        }
    }
    for (key, multi) in comp.multi_properties() {
        if key.eq_ignore_ascii_case(&pf.name) {
            props.extend(multi.iter());
        }
    }
    matches_prop_values(&props, pf)
}

/// Property exists (or is-not-defined). Any instance that satisfies optional
/// text-match, time-range, and all param-filters is enough.
#[cfg(feature = "caldav")]
fn matches_prop_values(props: &[&Property], pf: &PropertyFilter) -> bool {
    if pf.is_not_defined {
        return props.is_empty();
    }
    if props.is_empty() {
        return false;
    }
    props.iter().any(|prop| property_instance_matches(prop, pf))
}

#[cfg(feature = "caldav")]
fn property_instance_matches(prop: &Property, pf: &PropertyFilter) -> bool {
    if let Some(tm) = &pf.text_match
        && !text_matches_value(prop.value(), tm)
    {
        return false;
    }
    if let Some(tr) = &pf.time_range
        && !property_overlaps_time_range(prop, tr)
    {
        return false;
    }
    pf.param_filters
        .iter()
        .all(|param_f| param_filter_matches(prop, param_f))
}

/// DATE-TIME is a point; DATE is a one-day interval. Non-date values do not match.
#[cfg(feature = "caldav")]
fn property_overlaps_time_range(prop: &Property, tr: &TimeRange) -> bool {
    let Some(range) = parse_time_range_bounds(tr) else {
        return false;
    };
    let Some(dpt) = DatePerhapsTime::from_property(prop) else {
        return false;
    };
    let start = to_utc(&dpt);
    let end = match &dpt {
        DatePerhapsTime::Date(_) => start + TimeDelta::days(1),
        DatePerhapsTime::DateTime(_) => start,
    };
    time_spans_overlap(start, end, range.start, range.end)
}

#[cfg(feature = "caldav")]
fn param_filter_matches(prop: &Property, pf: &ParameterFilter) -> bool {
    let value = prop
        .params()
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(&pf.name))
        .map(|(_, p)| p.value());
    if pf.is_not_defined {
        return value.is_none();
    }
    let Some(value) = value else {
        return false;
    };
    match &pf.text_match {
        Some(tm) => text_matches_value(value, tm),
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

    fn vevent_ics(summary: &str, dtstart: &str, dtend: &str) -> String {
        format!(
            "BEGIN:VCALENDAR\nVERSION:2.0\nBEGIN:VEVENT\nUID:1\nDTSTART:{dtstart}\nDTEND:{dtend}\nSUMMARY:{summary}\nEND:VEVENT\nEND:VCALENDAR"
        )
    }

    #[test]
    fn prop_filter_time_range_on_dtstart() {
        let mut filter = vevent_filter();
        filter.prop_filters.push(PropertyFilter {
            name: "DTSTART".into(),
            is_not_defined: false,
            text_match: None,
            time_range: Some(TimeRange {
                start: Some("20240601T000000Z".into()),
                end: Some("20240701T000000Z".into()),
            }),
            param_filters: Vec::new(),
        });
        let query = vcalendar_with(filter);

        assert!(calendar_matches_query(
            &vevent_ics("Inside", "20240615T120000Z", "20240615T130000Z"),
            &query
        ));
        assert!(!calendar_matches_query(
            &vevent_ics("Outside", "20240101T120000Z", "20240101T130000Z"),
            &query
        ));
    }

    #[test]
    fn prop_filter_time_range_rejects_non_date_property() {
        let mut filter = vevent_filter();
        filter.prop_filters.push(PropertyFilter {
            name: "SUMMARY".into(),
            is_not_defined: false,
            text_match: None,
            time_range: Some(TimeRange {
                start: Some("20240601T000000Z".into()),
                end: Some("20240701T000000Z".into()),
            }),
            param_filters: Vec::new(),
        });
        let query = vcalendar_with(filter);
        assert!(!calendar_matches_query(
            &vevent_ics("June Event", "20240615T120000Z", "20240615T130000Z"),
            &query
        ));
    }

    #[test]
    fn prop_filter_param_filter_on_attendee_partstat() {
        let mut filter = vevent_filter();
        filter.prop_filters.push(PropertyFilter {
            name: "ATTENDEE".into(),
            is_not_defined: false,
            text_match: None,
            time_range: None,
            param_filters: vec![ParameterFilter {
                name: "PARTSTAT".into(),
                is_not_defined: false,
                text_match: Some(TextMatch {
                    text: "NEEDS-ACTION".into(),
                    ..Default::default()
                }),
            }],
        });
        let query = vcalendar_with(filter);

        let needs = "BEGIN:VCALENDAR\nVERSION:2.0\nBEGIN:VEVENT\nUID:1\nDTSTART:20240615T120000Z\nDTEND:20240615T130000Z\nSUMMARY:Needs\nATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:a@example.com\nEND:VEVENT\nEND:VCALENDAR";
        let accepted = "BEGIN:VCALENDAR\nVERSION:2.0\nBEGIN:VEVENT\nUID:2\nDTSTART:20240615T120000Z\nDTEND:20240615T130000Z\nSUMMARY:Accepted\nATTENDEE;PARTSTAT=ACCEPTED:mailto:b@example.com\nEND:VEVENT\nEND:VCALENDAR";
        assert!(calendar_matches_query(needs, &query));
        assert!(!calendar_matches_query(accepted, &query));
    }

    #[test]
    fn prop_filter_param_filter_on_dtstart_tzid() {
        let mut filter = vevent_filter();
        filter.prop_filters.push(PropertyFilter {
            name: "DTSTART".into(),
            is_not_defined: false,
            text_match: None,
            time_range: None,
            param_filters: vec![ParameterFilter {
                name: "TZID".into(),
                is_not_defined: false,
                text_match: Some(TextMatch {
                    text: "America/New_York".into(),
                    ..Default::default()
                }),
            }],
        });
        let query = vcalendar_with(filter);

        let with_tz = "BEGIN:VCALENDAR\nVERSION:2.0\nBEGIN:VEVENT\nUID:1\nDTSTART;TZID=America/New_York:20240615T120000\nDTEND;TZID=America/New_York:20240615T130000\nSUMMARY:NY\nEND:VEVENT\nEND:VCALENDAR";
        let utc = vevent_ics("UTC", "20240615T120000Z", "20240615T130000Z");
        assert!(calendar_matches_query(with_tz, &query));
        assert!(!calendar_matches_query(&utc, &query));
    }

    fn tzid_vevent_ics(tzid: &str, dtstart: &str, dtend: &str) -> String {
        format!(
            "BEGIN:VCALENDAR\nVERSION:2.0\nBEGIN:VEVENT\nUID:1\nDTSTART;TZID={tzid}:{dtstart}\nDTEND;TZID={tzid}:{dtend}\nSUMMARY:Zoned\nEND:VEVENT\nEND:VCALENDAR"
        )
    }

    fn vevent_time_range_query(start: &str, end: &str) -> CalendarQuery {
        let mut filter = vevent_filter();
        filter.time_range = Some(TimeRange {
            start: Some(start.into()),
            end: Some(end.into()),
        });
        vcalendar_with(filter)
    }

    #[test]
    fn tzid_date_time_is_converted_to_utc_in_time_range() {
        // EDT (UTC-4): 12:00-13:00 local is 16:00Z-17:00Z.
        let summer = tzid_vevent_ics("America/New_York", "20240615T120000", "20240615T130000");
        assert!(calendar_matches_query(
            &summer,
            &vevent_time_range_query("20240615T160000Z", "20240615T170000Z")
        ));
        assert!(!calendar_matches_query(
            &summer,
            &vevent_time_range_query("20240615T110000Z", "20240615T123000Z")
        ));

        // EST (UTC-5): 12:00-13:00 local is 17:00Z-18:00Z.
        let winter = tzid_vevent_ics("America/New_York", "20240115T120000", "20240115T130000");
        assert!(calendar_matches_query(
            &winter,
            &vevent_time_range_query("20240115T170000Z", "20240115T180000Z")
        ));
        assert!(!calendar_matches_query(
            &winter,
            &vevent_time_range_query("20240115T160000Z", "20240115T170000Z")
        ));
    }

    #[test]
    fn unknown_tzid_falls_back_to_wall_clock_as_utc() {
        let ics = tzid_vevent_ics("Custom/Office", "20240615T120000", "20240615T130000");
        assert!(calendar_matches_query(
            &ics,
            &vevent_time_range_query("20240615T120000Z", "20240615T130000Z")
        ));
        assert!(!calendar_matches_query(
            &ics,
            &vevent_time_range_query("20240615T160000Z", "20240615T170000Z")
        ));
    }

    #[test]
    fn tzid_vevent_busy_interval_is_in_utc() {
        let start = parse_caldav_date_time("20240601T000000Z").unwrap();
        let end = parse_caldav_date_time("20240701T000000Z").unwrap();
        let cal = parse_ics(&tzid_vevent_ics(
            "America/New_York",
            "20240615T120000",
            "20240615T130000",
        ));
        assert_eq!(
            calendar_busy_intervals(&cal, start, end),
            vec![(
                parse_caldav_date_time("20240615T160000Z").unwrap(),
                parse_caldav_date_time("20240615T170000Z").unwrap(),
            )]
        );
    }

    fn dav_path(p: &str) -> DavPath {
        DavPath::new(p).unwrap()
    }

    #[test]
    fn immediate_calendar_child_is_collection() {
        assert!(is_path_in_caldav_directory(&dav_path(
            "/calendars/my-calendar"
        )));
        assert!(is_path_in_caldav_directory(&dav_path(
            "/calendars/my-calendar/"
        )));
    }

    #[test]
    fn nested_and_home_paths_are_not_calendars() {
        for p in [
            "/calendars",
            "/calendars/",
            "/calendars/my-calendar/event.ics",
            "/calendars/my-calendar/nested",
            "/calendars/my-calendar/nested/",
            "/other",
            "/",
        ] {
            assert!(
                !is_path_in_caldav_directory(&dav_path(p)),
                "{p} must not be a calendar collection"
            );
        }
    }

    #[test]
    fn calendar_path_uses_prefix_stripped_bytes() {
        let mut path = dav_path("/dav/calendars/my-calendar");
        path.set_prefix("/dav").unwrap();
        assert!(is_path_in_caldav_directory(&path));

        let mut nested = dav_path("/dav/calendars/my-calendar/event.ics");
        nested.set_prefix("/dav").unwrap();
        assert!(!is_path_in_caldav_directory(&nested));
    }

    fn parse_ics(ics: &str) -> Calendar {
        validate_calendar_data(ics).expect("valid iCalendar")
    }

    fn jan_range() -> (DateTime<Utc>, DateTime<Utc>) {
        (
            parse_caldav_date_time("20240101T000000Z").unwrap(),
            parse_caldav_date_time("20240201T000000Z").unwrap(),
        )
    }

    #[test]
    fn freebusy_bounds_require_valid_start_and_end() {
        assert!(
            parse_freebusy_bounds(&TimeRange {
                start: Some("20240101T000000Z".into()),
                end: Some("20240201T000000Z".into()),
            })
            .is_some()
        );
        assert!(
            parse_freebusy_bounds(&TimeRange {
                start: None,
                end: Some("20240201T000000Z".into()),
            })
            .is_none()
        );
        assert!(
            parse_freebusy_bounds(&TimeRange {
                start: Some("not-a-date".into()),
                end: Some("20240201T000000Z".into()),
            })
            .is_none()
        );
    }

    #[test]
    fn opaque_vevent_in_range_is_busy() {
        let (start, end) = jan_range();
        let cal = parse_ics(&vevent_ics("Busy", "20240101T120000Z", "20240101T130000Z"));
        assert_eq!(
            calendar_busy_intervals(&cal, start, end),
            vec![(
                parse_caldav_date_time("20240101T120000Z").unwrap(),
                parse_caldav_date_time("20240101T130000Z").unwrap(),
            )]
        );
    }

    #[test]
    fn transparent_vevent_is_not_busy() {
        let (start, end) = jan_range();
        let ics = "BEGIN:VCALENDAR\nVERSION:2.0\nBEGIN:VEVENT\nUID:1\nDTSTART:20240101T120000Z\nDTEND:20240101T130000Z\nTRANSP:TRANSPARENT\nEND:VEVENT\nEND:VCALENDAR";
        let cal = parse_ics(ics);
        assert!(calendar_busy_intervals(&cal, start, end).is_empty());
    }

    #[test]
    fn vfreebusy_period_is_busy_when_overlapping() {
        let (start, end) = jan_range();
        let ics = "BEGIN:VCALENDAR\nVERSION:2.0\nBEGIN:VFREEBUSY\nUID:fb\nFREEBUSY:20240101T150000Z/20240101T160000Z\nEND:VFREEBUSY\nEND:VCALENDAR";
        let cal = parse_ics(ics);
        assert_eq!(
            calendar_busy_intervals(&cal, start, end),
            vec![(
                parse_caldav_date_time("20240101T150000Z").unwrap(),
                parse_caldav_date_time("20240101T160000Z").unwrap(),
            )]
        );
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
