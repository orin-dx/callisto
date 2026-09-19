---
callisto-cli: minor
---

**Recoverable, validated product releases**

`callisto release` now recovers interrupted releases from provider observation (registry, remote tag, forge release, assets) and validates arguments and the receipt path before any effect. One profile authority governs execution (E198). `previous-tag-templates` keeps renamed tags discoverable, and the CLI product tag is now `callisto@{version}`. The `[release]` table configures product artifacts and a qualified `product-package`, with canonical destination isolation and per-command deadlines. Sources without a `[release]` plan get zero artifact slots and a notice.
