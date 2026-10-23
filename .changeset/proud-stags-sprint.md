---
callisto-cli: patch
---

status, version and release --dry-run read tags and manifests in a handful of git calls instead of several per package, so they take well under a second on workspaces with hundreds of packages.
