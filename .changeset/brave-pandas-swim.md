---
callisto-cli: patch
---

**Package directories with unusual names release correctly.** A package directory named with `*`, `?` or leading spaces is now read as exactly that directory.

- Commit inference and the version commit no longer pick up a sibling such as `packages/odd-other` for a package in `packages/odd*`.
- A commit message containing the U+001E control character no longer stops inference with `E026`.
- Resuming an interrupted `version` run in a workspace directory with leading or trailing spaces resumes that workspace, not a similarly named sibling.
