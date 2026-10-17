---
callisto-cli: minor
---

Callisto no longer plans a release from a workspace it could only partly read. A package whose manifest cannot be opened (for example a member that inherits its version from a Rust workspace Callisto cannot find) used to keep no publish target and no dependency edges, so it never published and its dependents never cascaded, without a message. Every command now stops with a coded error naming each such manifest.
