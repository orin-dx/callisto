---
callisto-cli: minor
---

A Python release that stopped partway is completed on rerun. Only the files PyPI lacks are uploaded, and the release counts as published only when both the wheel and the sdist it builds are on the index, so no tag is created for a half-published release. Uploads name their files instead of a dist glob, so a stale dist directory is no longer published.

Breaking: `callisto_graph`'s `registry_argv::pypi_publish_argv` is replaced by `pypi_build_argv` and `pypi_upload_argv`.
