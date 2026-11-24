---
callisto-cli: patch
---

**Tags go to exactly the repository `origin` names**

`callisto release` now pushes and checks tags at the `origin` push URL as configured, instead of a rewritten one.

- Before, `https://host/team/repo` became `https://host/team/repo.git`, and `git@host:team/repo` became `ssh://git@host/team/repo.git`: on a self-hosted Git server these can be different repositories.
- On GitHub, `owner/repo` and `owner/repo.git` still name the same repository for releases and changelog links.
- An `origin` URL with whitespace, a backslash, or a `.` or `..` path segment is now refused as not hosted.
