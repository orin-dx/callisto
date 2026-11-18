---
callisto-cli: minor
---

**Changesets v3 prerelease cycles work.** A repository mid-cycle with a v3 `pre.json` (only `mode` and `tag`) now versions, reruns and exits instead of failing with E114.

- Applied changesets move to `.changeset/pre/`, as Changesets v3 does, and are released together at `pre exit`.
- `pre.json` is written back in the shape it was read, v2 or v3, and keeps keys Callisto does not know. A cycle Callisto starts with `pre enter` still writes v2.
