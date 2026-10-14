---
callisto-cli: patch
---

Releases that ship artifacts no longer fail with E140: the GitHub CLI that checks attestations and publishes the release keeps its own token instead of running unauthenticated.
