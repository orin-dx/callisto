---
callisto-graph: patch
---

gh attestation verify now retries through the same bounded observation policy as every other GitHub-touching release check, instead of hard-failing the release on one transient timeout or I/O error.
