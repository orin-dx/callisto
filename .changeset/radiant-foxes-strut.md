---
callisto-cli: minor
---

When a run adopts a Cargo or npm version that is already published, Callisto now warns if the registry records a different repository for it than this repository's origin, since the name may belong to someone else's package. It still adopts the version. A registry that records no repository, and PyPI, are not checked.
