---
callisto-cli: patch
---

The docs and specs now say what a bump does to a Cargo member that inherits its version (`version.workspace = true`): it gets a literal version of its own, and `[workspace.package]` is left as it is. E248, which no command emitted, is retired.
