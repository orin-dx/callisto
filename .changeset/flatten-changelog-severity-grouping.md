---
callisto-changelog: minor
---

**Changelog entries no longer split by severity within one version**

- **Breaking:** removed `callisto_changelog::group_entries`/`GroupedEntries` — no replacement, since severity grouping no longer happens.
- `CHANGELOG.md` entries for one version now render as a single flat bullet list instead of separate `### Major Changes`/`### Minor Changes`/`### Patch Changes` sub-sections. The version heading already states the one, real applied bump — resurfacing each entry's own originally-authored severity underneath it added confusion without adding information, especially for a package bumped solely by fixed-group convergence.
