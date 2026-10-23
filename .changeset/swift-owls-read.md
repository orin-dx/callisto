---
callisto-cli: patch
---

`status`, `version` and `release` skip two history reads for tags made by a recent release. They still read them for every other tag, so older and hand-made tags behave as before.
