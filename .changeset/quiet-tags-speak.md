---
callisto-cli: patch
---

`release` and `release --dry-run` now warn when a package's last tag exists in your repository but not on `origin`. A release that tagged locally and then failed to push used to be reported as "Nothing to release." on the next commit, with the remote tag and the GitHub release never created.
