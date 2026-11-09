---
callisto-cli: minor
---

The release PR opens with a "Read before merging" block, so nothing important is missed at merge time. It lists:

- `Breaking:` and `Action:` notes from your changesets, and a `Breaking:` note on a change whose bump is too low for it
- packages that changed since their last release but are not part of this one

Start a changeset paragraph with `Breaking:` or `Action:` to have it shown there.
