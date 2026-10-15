---
callisto-cli: patch
---

A release now stops with a clear error when git cannot read the local tag state, for example a corrupt or unreadable repository, or when npm cannot be reached or rejects the version lookup, instead of treating the version as unpublished and attempting to publish or tag. npm 8 and later still report a missing version as unpublished.
