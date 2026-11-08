---
callisto-cli: minor
---

The release PR body now has a "Read before merging" callout. It lists each `Breaking:` and `Action:` paragraph from a changeset, and the packages whose source changed since their last release but that the release leaves out. A breaking paragraph removes the "backward compatible" claim.
