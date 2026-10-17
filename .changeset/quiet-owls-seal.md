---
callisto-cli: minor
---

A release workflow that builds Python distributions in a job without registry credentials now publishes exactly those files. The generated build-matrix workflow gains a `build-python` job, the manifest records each file's name, size and sha256, and a file PyPI already holds under a sealed name with different bytes stops the release before any upload. Python builds also set `SOURCE_DATE_EPOCH` to the release commit time.
