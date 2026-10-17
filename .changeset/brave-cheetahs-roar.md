---
callisto-cli: minor
---

A Python release that stopped partway is completed on rerun. Only the files PyPI lacks are uploaded, and the release counts as published only when both the wheel and the sdist it builds are on the index, so a tag is no longer created for a half-published release. Uploads name their files instead of a dist glob, so a stale dist directory is no longer published, and the build goes to a temporary directory. A file this run uploaded must match the digest PyPI serves, and a build that produces no wheel or sdist fails with a clear error. Breaking: callisto_graph's registry_argv::pypi_publish_argv is replaced by pypi_build_argv and pypi_upload_argv.
