---
callisto-cli: patch
---

Fix version rerun without a commit in between double-logging a changelog entry in pre-release mode, and now refuse the same ordinary add-then-version-twice sequence outside pre mode as a partial run.
