---
callisto-cli: patch
---

**E180 for GitHub's tag-push workflow guard**

When GitHub refuses a tag push from `GITHUB_TOKEN` (the tagged commit's workflows differ from every branch tip, e.g. when recovering an older release), `callisto release` reports E180 with the tag and the fix: push the tags with a PAT or deploy key, then re-run the release. Previously a generic E164.
