---
callisto-cli: patch
---

`callisto release` no longer tags a version that is already on the registry onto a later commit. After a run that stopped between publishing and tagging, a rerun on a newer commit that changed the package now stops with E320 and names the commit that set the version. Checking that commit out and running `callisto release` again adopts the published version and tags it there.
