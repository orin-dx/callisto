---
callisto-cli: patch
---

npm ranges with spaced comparators are now recognized. If a new dependency version falls outside the range, `version` stops and asks you to update it instead of silently leaving the dependent unchanged.
