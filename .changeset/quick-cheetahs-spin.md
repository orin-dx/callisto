---
callisto-cli: patch
---

Package selectors (--package, add, matrix, release, config rules) now resolve consistently by any registered ecosystem-qualified or native manifest name; a mismatched-ecosystem rule or selector no longer silently matches the wrong package, and a package-set rule like `cargo/foo-*` now correctly restricts matches to Cargo packages instead of matching any ecosystem.
