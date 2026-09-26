---
callisto-cli: patch
---

Release checks against crates.io, npm, PyPI, GitHub and the git remote retry on timeouts and connection failures, and a PyPI rate limit waits for `Retry-After`, instead of failing the release.
