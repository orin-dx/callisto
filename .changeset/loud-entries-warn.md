---
callisto-cli: patch
---

`callisto version` now warns about a changeset entry that names a package that is not in the workspace, as `status` already did. The changeset stays on disk, and its other entries are applied again by the next `version` run, so fix or remove the entry.
