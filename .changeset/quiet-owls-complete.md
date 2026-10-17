---
callisto-cli: minor
---

Callisto no longer plans a release from a workspace it could only partly read. A package whose manifest cannot be opened (for example a member that inherits its version from a Rust workspace Callisto cannot find) used to be dropped from the dependency graph and from publishing without a message. `version`, `release` and the other commands now stop with a coded error naming each such manifest, and `status` reports it as an error so `status --check` fails.
