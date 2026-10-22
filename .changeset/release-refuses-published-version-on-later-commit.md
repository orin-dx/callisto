---
callisto-cli: patch
---

`callisto release` now stops with E320 instead of tagging a version that is already on the registry onto a later commit. After a run that stopped between publishing and tagging, a rerun on a newer commit that changed the package names the commit that set the version; checking that commit out and running `callisto release` again adopts the published version and tags it there. The check follows the commit that set the package's own literal version, so a version inherited from the workspace is not covered.
