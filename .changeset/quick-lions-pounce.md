---
callisto-cli: patch
---

A duration with a non-ASCII unit, such as a confirm-timeout of 5é, now fails with a clear error instead of crashing. A Git failure while inferring versions from commits now stops the command instead of being skipped.
