---
callisto-cli: patch
---

A rerun after a failed tag push now completes the release. A tag that exists on the release commit no longer hides unfinished work: the push, GitHub release and any other missing steps are performed and the receipt is written.
