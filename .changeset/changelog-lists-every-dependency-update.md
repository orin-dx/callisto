---
callisto-cli: patch
---

A package released because its dependencies changed now lists every one of them under "Dependency updates" in its changelog, each at the version it actually reached.

- Before, only the first dependency that bumped the package was listed. A package with its own changeset got no dependency line at all, though its manifest pins were rewritten.
- A fixed group could also leave the line one version behind the manifest (`b → 1.0.1` while the pin said `=1.1.0`).
- You do not need to do anything. Changelog sections already written are not changed.
