---
callisto-cli: patch
---

Notes about packages Callisto skips are printed once per run.

- Before, a command that loads the workspace more than once, such as `callisto init`, printed the same note up to three times.
- This covers the notes for a crate with no version and for an ignored private workspace root.
