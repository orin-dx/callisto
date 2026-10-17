---
callisto-cli: patch
---

A failed Git lookup is now an error instead of "nothing there". A broken or missing repository used to read as no tags, no such ref, no file at that revision or no matches, so `status` and `version` could plan from the wrong history without saying so. Only Git's own "not found" answer counts as absent now, a tag that is listed but does not resolve to a commit stops the run, and `status` warns once about each tag that matches a package's tag template but is not a version.
