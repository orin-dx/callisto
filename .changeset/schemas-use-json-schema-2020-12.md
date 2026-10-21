---
callisto-cli: minor
---

`callisto schema` now prints JSON Schema draft 2020-12: shared definitions live under `$defs` instead of `definitions`, and a fixed string variant is written as `const` instead of a one-element `enum`. The shapes the schemas accept are unchanged.
