---
callisto-cli: patch
---

A local release no longer tags a published version onto code that was never published when the package inherits its version from the workspace, has more than 100 edits to its manifest, or was merged after unrelated commits; a registry that cannot be read now stops the release. Signed commits no longer break change detection when log.showSignature is set.
