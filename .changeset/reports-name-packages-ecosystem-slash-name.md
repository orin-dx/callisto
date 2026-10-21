---
callisto-cli: minor
---

The `status`, `version`, `snapshot` and `matrix` JSON reports now name every package `ecosystem/name`, such as `cargo/foo`, for its primary ecosystem, whether or not another ecosystem shares the name. This covers bumps, cascade reasons, diagnostics and the `matrix` map keys. These four reports now carry `schemaVersion` 2; the other reports still carry 1. Text output follows. Changeset entries, `pre.json` keys and default tags are unchanged.
