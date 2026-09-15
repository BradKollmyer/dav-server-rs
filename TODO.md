# TODO list

Checked against the current tree (2026-09). Items that have landed
are noted as done so this file is a backlog, not a history.

## Protocol compliance

### Apply headers to every resource on COPY / MOVE / DELETE

RFC 4918 9.6.1 / 9.8.5 / 9.9.2: headers on the request MUST be applied
to every resource processed (except `Destination` on COPY/MOVE).

We apply `If-Match`, `If-None-Match`, `If-Modified-Since`,
`If-Unmodified-Since`, and `If` to the request URL only, not to
descendants on `Depth: infinity`.

### Per-resource lock checks on MOVE / DELETE

A conflicting lock on the request URL (or, with `Depth: infinity`,
anywhere under it) fails the whole request with `423 Locked`. RFC
partial-failure rules want a `207 Multi-Status` that skips only the
locked members.

That also means collection MOVE cannot stay a single `rename`; it
would have to walk resource-by-resource, like COPY.

Related: after a successful DELETE/MOVE we call `locksystem.delete`
once on the request URL. If the walk only partially succeeds, lock
entries for removed members can be left behind. See comments in
`handle_delete.rs` / `handle_copymove.rs`.

### COPY Depth 0 of a collection

RFC 4918 9.8.3: `Depth: 0` copies the collection and its properties,
not its members. We create the destination directory (or no-op if it
already exists) and do not copy properties.

### DELETE Depth 0 on collections

RFC 4918 9.6.1: a collection DELETE is always `Depth: infinity`; any
other Depth is `400`. We still accept `Depth: 0` (attempt `remove_dir`
without walking children).

### Symbolic links

`hide_symlinks` (default true) treats a symlink as 404 for GET,
PROPFIND, and mutating methods. The link itself is not a first-class
resource. First-class links would be [RFC 4437 redirectref](https://tools.ietf.org/html/rfc4437).

## Race conditions

RFC 4918 leaves overlap to the client via LOCK. We do not take an
internal lock around COPY / MOVE / DELETE, so two unlocked clients
can still race.

Optional hardening (not a compliance requirement):

- if we already hold an exclusive lock on the request URL, do nothing
- otherwise take a short-lived exclusive Depth: infinity lock on the
  request URL without checking existing locks
- then check whether we can actually lock the URL and descendants
- if not, unlock and fail; if yes, do the work and unlock
- timeout on the order of 10s, refresh every ~5s, so a crash does not
  leave a stuck lock when the lock database is separate from the server

## Improvements

- Restrict fake locking to the user-agents that need it (Nextcloud's
  list): `/WebDAVFS/` (Apple), `/Microsoft Office OneNote 2013/`,
  `/^Microsoft-WebDAV/`. `FakeLs` is currently all-or-nothing; a real
  `MemLs` plus UA-gated fake LOCK would let Windows/macOS mount while
  still enforcing locks for other clients. `(WebDAVFS|Microsoft)` is
  probably enough.

- API: move the filesystem interface to `Path` / `PathBuf` and hide
  `DavPath`. Breaking for every `DavFileSystem` implementor. LocalFs
  already converts internally.

- CardDAV has no dedicated README (CalDAV does: `README.CalDAV.md`).

## Tests (remaining)

Actix and Warp shims have cargo tests (`tests/actix_compat.rs`,
`tests/warp_compat.rs`). The Axum example has none.

Litmus against `sample-litmus-server` is still the live-server check
for RFC 4918 basic / copymove / props / locks / http.

## Live properties

### Done

- `X-OC-MTime` / `X-OC-CTime` on PUT and MKCOL (`set_modified` /
  `set_created`; response `accepted` when the backend takes it)
- `Win32LastModifiedTime` on PROPPATCH via `set_modified` (207 still
  reports 200 so Windows Explorer keeps working; malformed is 409)
- PROPFIND of `Win32CreationTime`, `Win32LastAccessTime`
- PROPFIND of `Win32FileAttributes`: hidden (name starts with `.`),
  directory (`0x10`), archive/file (`0x20`)
- PROPFIND of Apache `executable` (unix mode)

### Remaining

- LocalFs dead properties. XFS has scalable xattrs (ext2/3/4 max 4KB).
  On XFS we could also store `creationdate`. MemFs already has dead
  props; LocalFs `have_props` is false.
- PROPPATCH Apache `executable` (toggle the unix execute bit). GET
  works; SET is `403`.
- PROPPATCH `Win32LastAccessTime` for real. `DavFileSystem::set_accessed`
  exists but is not wired; SET currently reports 200 without applying.
- `Win32FileAttributes` readonly (`0x1`) needs `permissions()` on
  `DavMetaData`. Do not implement readonly on directories (Windows
  means "all files in the directory"). SET of FileAttributes also
  reports 200 without applying.
- PROPPATCH `DAV:getlastmodified` is `403`; mtime is set via
  `X-OC-MTime` / `Win32LastModifiedTime` instead.
- Some servers allow SET of `DAV:getcontentlength`. We do not, and
  probably should not.

## CalDAV / CardDAV

Partial RFC 4791 / RFC 6352. Remaining:

- `free-busy-query` is `501`
- no scheduling (iTIP / iMIP)
- limited `calendar-user-principal` / no sharing
- no recurrence expansion in `calendar-query`
- calendar / addressbook identity is only immediate children of
  `/calendars` and `/addressbooks`
- basic timezone handling

See `README.CalDAV.md` for the CalDAV subset that *is* implemented.

## Project ideas

- [RFC 4437 WebDAV Redirect Reference Resources](https://tools.ietf.org/html/rfc4437)
- [RFC 3744 WebDAV ACL](https://tools.ietf.org/html/rfc3744)

Litmus also has RFC 5842 "bind" and RFC 3253 "versioning". We do not
support those.

## Not going to happen

### Compression

- compressed PROPFIND responses
- compressed PUT bodies

No WebDAV client we know of uses this.
