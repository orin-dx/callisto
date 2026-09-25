---
callisto-cli: patch
---

`init --artifact-target a,,b` (flag or interactive) errors naming the empty target instead of silently dropping it.
The PyPI real-registry e2e test now builds offline (`--no-isolation` against the CI venv pinned setuptools/wheel); no network fetch during `python -m build`.
`status --check` reports an ambiguous bare package name in a changeset as the `AmbiguousPackageName` diagnostic (exit 1) instead of hard-erroring.
