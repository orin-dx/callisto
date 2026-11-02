---
callisto-cli: patch
---

`callisto status --check --strict` now fails on the same warnings as `callisto version --strict`, such as a napi target missing from its fixed group. Without `--strict` they stay warnings.
