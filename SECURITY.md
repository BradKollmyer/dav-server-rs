# Security Policy

## Supported versions

Security fixes are made on the default branch (`main`) and shipped in the next
`dav-server` crate release. Older published versions are not patched on their
own release lines.

## Reporting a vulnerability

Report security issues privately through GitHub:

https://github.com/BradKollmyer/dav-server-rs/security/advisories/new

Do not open a public issue, pull request, or discussion for a vulnerability.

Include:

- A description of the issue and its impact
- Steps to reproduce, or a proof of concept
- Affected versions or commits, if known

You should receive an acknowledgement. Confirmed reports are handled as a
draft GitHub Security Advisory. Credit is included in the advisory if you
want it.

## Scope

This policy covers the `dav-server` library in this repository: the
WebDAV/CalDAV/CardDAV handler, filesystem backends, lock systems, and HTTP
adapter modules.

In scope includes path traversal, authentication or authorization bypass,
XML entity expansion, lock bypass, and cross-collection data leaks.

Out of scope:

- Bugs that exist only in example servers, unless they show a library defect
- Vulnerabilities in dependencies that do not affect this crate (report those
  upstream; Dependabot tracks them here)
