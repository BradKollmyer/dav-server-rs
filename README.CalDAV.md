# CalDAV Support in dav-server

This document describes the CalDAV (Calendaring Extensions to WebDAV) support in the dav-server library.

## Overview

CalDAV is an extension of WebDAV that provides a standard way to access and manage calendar data over HTTP. It's defined in [RFC 4791](https://tools.ietf.org/html/rfc4791) and allows calendar clients to:

- Create and manage calendar collections
- Store and retrieve calendar events, tasks, and journals
- Query calendars with complex filters
- Synchronize calendar data between clients and servers

## Features

The CalDAV implementation in dav-server includes:

- **Calendar Collections**: Immediate children of `/calendars` (`/calendars/<name>`), each containing `.ics` files
- **MKCALENDAR Method**: Create new calendar collection
- **REPORT Method**: Implemented subset of `calendar-query` and `calendar-multiget` (see below)
- **CalDAV Properties**: Calendar-specific WebDAV properties (`max-resource-size` is 1MB)
- **Time Range Queries**: Filter events by date/time ranges
- **Component Filtering**: Nested `comp-filter` by calendar component types (VEVENT, VTODO, etc.)
- **Property Filtering**: `prop-filter` with optional `text-match`, `time-range`, and `param-filter`
- **Unreadable members**: Unreadable or unparseable collection members are skipped, not failed

## Enabling CalDAV

CalDAV support is available as an optional cargo feature:

```toml
[dependencies]
dav-server = { version = "0.12", features = ["caldav"] }
```

## Quick Start

Here's a basic CalDAV server setup:

```rust
use dav_server::{DavHandler, fakels::FakeLs, localfs::LocalFs};

let server = DavHandler::builder()
    .filesystem(LocalFs::new("/dav_files", false, false, false))
    .locksystem(FakeLs::new())
    .build_handler();
```
## Important Setup Notes

**CalDAV Directory Creation**: The `/calendars` directory (defined in `dav_server::caldav::DEFAULT_CALDAV_DIRECTORY`) must exist before CalDAV operations. `MemFs` and `LocalFs` create it automatically, but custom `GuardedFileSystem` implementations must initialize it during startup. Calendar identity is `/calendars/<name>`: only immediate children of `/calendars` are calendar collections.

## CalDAV Methods

### MKCALENDAR

Creates a new calendar collection:

```bash
curl -X MKCALENDAR http://localhost:8080/calendars/my-calendar/
```

With properties:

```bash
curl -X MKCALENDAR http://localhost:8080/calendars/my-calendar/ \
  -H "Content-Type: application/xml" \
  --data '<?xml version="1.0" encoding="utf-8" ?>
<C:mkcalendar xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:set>
    <D:prop>
      <D:displayname>My Calendar</D:displayname>
      <C:calendar-description>Personal calendar</C:calendar-description>
    </D:prop>
  </D:set>
</C:mkcalendar>'
```

### REPORT

Query calendar data. Response `href` values include the handler `strip_prefix`
(the mount prefix). Unreadable or unparseable members are omitted from
`calendar-query` results.

`calendar-query` requires a `CALDAV:filter` with a nested `comp-filter`
(RFC 4791 7.8.1). A REPORT without that filter is `400 Bad Request`. Nested
`comp-filter` elements are evaluated; `prop-filter` matches the named iCalendar
property, with optional `text-match`, `time-range` on DATE/DATE-TIME values,
and `param-filter` on property parameters.

#### Calendar Query

```bash
curl -X REPORT http://localhost:8080/calendars/my-calendar/ \
  -H "Content-Type: application/xml" \
  -H "Depth: 1" \
  --data '<?xml version="1.0" encoding="utf-8" ?>
<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop>
    <C:calendar-data/>
  </D:prop>
  <C:filter>
    <C:comp-filter name="VCALENDAR">
      <C:comp-filter name="VEVENT">
        <C:time-range start="20240101T000000Z" end="20241231T235959Z"/>
      </C:comp-filter>
    </C:comp-filter>
  </C:filter>
</C:calendar-query>'
```

#### Calendar Multiget

```bash
curl -X REPORT http://localhost:8080/calendars/my-calendar/ \
  -H "Content-Type: application/xml" \
  --data '<?xml version="1.0" encoding="utf-8" ?>
<C:calendar-multiget xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop>
    <C:calendar-data/>
  </D:prop>
  <D:href>/calendars/my-calendar/event1.ics</D:href>
  <D:href>/calendars/my-calendar/event2.ics</D:href>
</C:calendar-multiget>'
```

`calendar-multiget` hrefs must name calendar object resources inside the
target collection (RFC 4791 7.9). Href values outside that collection are
reported as missing (404 in the 207), not fetched.

## CalDAV Properties

The implementation supports standard CalDAV properties:

### Collection Properties

- `calendar-description`: Human-readable description
- `calendar-timezone`: Default timezone for the calendar
- `supported-calendar-component-set`: Supported component types (VEVENT, VTODO, etc.)
- `supported-calendar-data`: Supported calendar data formats
- `max-resource-size`: Maximum size for calendar resources (1MB)

### Principal Properties

- `calendar-home-set`: URL of the user's calendar home collection
- `calendar-user-address-set`: Calendar user's addresses
- `schedule-inbox-URL`: URL for scheduling messages
- `schedule-outbox-URL`: URL for outgoing scheduling

## Working with Calendar Data

### Adding Events

Store iCalendar data using PUT. Invalid iCalendar data in a calendar collection is rejected with 403 (`validate_calendar_data`):

```bash
curl -X PUT http://localhost:8080/calendars/my-calendar/event.ics \
  -H "Content-Type: text/calendar" \
  --data 'BEGIN:VCALENDAR
VERSION:2.0
PRODID:-//Example Corp//CalDAV Client//EN
BEGIN:VEVENT
UID:12345@example.com
DTSTART:20240101T120000Z
DTEND:20240101T130000Z
SUMMARY:New Year Meeting
DESCRIPTION:Planning meeting for the new year
END:VEVENT
END:VCALENDAR'
```

### Retrieving Events

Use GET to retrieve individual calendar resources:

```bash
curl http://localhost:8080/calendars/my-calendar/event.ics
```

## Client Compatibility

The CalDAV implementation has been tested with:

- **Thunderbird**: Full support for calendar sync
- **Apple Calendar**: Compatible with basic operations
- **CalDAV-Sync (Android)**: Works with standard CalDAV features
- **Evolution**: Support for calendar collections and events

## Limitations

Current limitations include:

- Partial RFC 4791 coverage: `calendar-query` / `calendar-multiget` are an implemented subset (nested `comp-filter`, `prop-filter` `text-match`/`time-range`/`param-filter`, required filter, href confinement)
- `free-busy-query` returns `501 Not Implemented`
- Calendar identity is `/calendars/<name>` (immediate children of `/calendars` only)
- `max-resource-size` is 1MB
- PUT or PATCH of invalid iCalendar data into a calendar collection is 403 (`validate_calendar_data`); a failed partial write is restored or removed
- No scheduling support (iTIP/iMIP)
- Limited calendar-user-principal support
- No calendar sharing or ACL support
- Basic time zone handling
- No recurring event expansion in queries
- `calendar-multiget` hrefs outside the collection are treated as missing

## Example Applications
These calendar server examples lacks authentication and does not support user-specific access. The default FileSystems can only create collections on the path "/calendars".  
For a production environment, you should implement the GuardedFileSystem for better security and user management.

### Calendar Server

```rust
use dav_server::{DavHandler, fakels::FakeLs, localfs::LocalFs};
use std::net::SocketAddr;

#[tokio::main]
async fn main() {
    let server = DavHandler::builder()
        .filesystem(LocalFs::new("/calendars", false, false, false))
        .locksystem(FakeLs::new())
        .build_handler();

    // Serve on port 8080
    // Calendars accessible at http://localhost:8080/calendars/
}
```

### Multi-tenant Calendar Service

```rust
use dav_server::{DavHandler, memfs::MemFs, memls::MemLs};

// Use in-memory filesystem for demonstration
let server = DavHandler::builder()
    .filesystem(MemFs::new())
    .locksystem(MemLs::new())
    .principal("/principals/user1/")
    .build_handler();
```

## Testing

Run CalDAV tests with:

```bash
cargo test --features caldav --test caldav_tests
```

Or the full suite, including CardDAV:

```bash
cargo test --all-features
```

Run the CalDAV example:

```bash
cargo run --example caldav --features caldav
```

## Standards Compliance

This is a partial implementation of:

- [RFC 4791](https://tools.ietf.org/html/rfc4791) - Calendaring Extensions to WebDAV (CalDAV)
- [RFC 5545](https://tools.ietf.org/html/rfc5545) - Internet Calendaring and Scheduling Core Object Specification (iCalendar)
- [RFC 4918](https://tools.ietf.org/html/rfc4918) - HTTP Extensions for Web Distributed Authoring and Versioning (WebDAV)

## Contributing

Contributions to improve CalDAV support are welcome. Areas for enhancement include:

- Scheduling support (iTIP)
- Additional client compatibility testing