---
callisto-cli: patch
---

pre enter after pre exit resumes the cycle under the new tag instead of failing with a stale already-in-pre-release-mode error; pre exit with no pre.json now reports a clear fix instead of a raw I/O error
