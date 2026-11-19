---
callisto-cli: minor
---

**Changesets 3.x prerelease cycles work.** A repository mid-cycle with the `pre.json` that `@changesets/cli` 3.x writes (only `mode` and `tag`) now versions, reruns and exits instead of failing with E114.

- Applied changesets move to `.changeset/pre/`, as Changesets v3 does, and are released together at `pre exit`.
- `pre.json` is written back in the shape it was read, v2 or v3, and keeps keys Callisto does not know. A cycle Callisto starts with `pre enter` still writes v2.
- The E114, E115 and E243 help no longer suggests deleting `pre.json` and re-entering, which would drop a cycle's archived notes.
