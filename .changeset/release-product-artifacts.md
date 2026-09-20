---
callisto-cli: minor
---

**Recoverable, validated product releases**

`callisto release` now recovers interrupted releases from provider observation (registry, remote tag, forge release, assets) and validates arguments and the receipt path before any effect. One profile authority governs execution (E198). `previous-tag-templates` keeps renamed tags discoverable, and the CLI product tag is now `callisto@{version}`. The `[release]` table configures product artifacts and a qualified `product-package`, with canonical destination isolation and per-command deadlines. Sources without a `[release]` plan get zero artifact slots and a notice. Registry observation now reads the registry's own protocol (cargo sparse index, PyPI JSON) rather than shelling to `cargo info`, so a yanked version is reported as a conflict instead of an absence and a PyPI publish can produce a receipt; transient registry and forge failures (429, 5xx, rate-limited 403, timeouts) are retried with bounded exponential backoff that honours `Retry-After`, instead of aborting the release.
