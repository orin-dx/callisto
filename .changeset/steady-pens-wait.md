---
callisto-cli: patch
---

A changeset with an entry that names no package in the workspace, such as a typo or a removed package, now applies none of its entries and says so. Before, its other entries were applied and the file stayed on disk, so every `version` run bumped those packages again. A Cargo crate with no version, such as an xtask, no longer stops every run with E013: it is skipped with a note.
