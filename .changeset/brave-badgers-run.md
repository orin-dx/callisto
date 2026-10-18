---
callisto-cli: minor
---

A `pypi` registry in `callisto.toml` can set `publish-url` when its upload endpoint differs from its simple index: Callisto reads releases from `url` and uploads to `publish-url`. Without it, `url` serves both, as before.
