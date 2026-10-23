---
callisto-cli: patch
---

A release no longer stops with E319 when a dependent's range already covers the new version but cannot be raised, such as `*` or a bare Python requirement. A range that a rewrite would leave excluding the new version, such as `<2.0.0` for 2.0.0, now fails with E319 instead of being published.
