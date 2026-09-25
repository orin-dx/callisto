---
callisto-graph: patch
---

Release observation retries now survive command timeouts and I/O failures (cargo/npm/PyPI/gh/git), and PyPI observation honors HTTP 429/5xx Retry-After like the GitHub forge lookup does.
