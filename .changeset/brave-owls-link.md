---
callisto-cli: minor
---

A package that sits behind a symbolic link is now rejected with an error naming it, because Git cannot stage a change to a file beneath a link and the version bump could never be committed. A link that holds no package is ignored, and a link can be skipped by listing it in a `.gitignore` under the workspace root.
