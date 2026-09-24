---
callisto-cli: minor
---

**`callisto release` replaces the legacy publish commands**

`callisto release` publishes, tags, and creates GitHub releases for every package whose current version has no tag, from any branch with a clean worktree. `--dry-run` previews the plan anywhere; `--package` restricts it; credentials are checked before the first effect; the receipt goes to the platform state dir or `--receipt`.

Breaking:
- Removed `callisto publish`, `plan-publish`, `tag` (including `--floating-major`), and `filter-plan`. Use `callisto release` or `callisto release --dry-run`.
- Removed `schema --type tag` and `schema --type plan-publish`.
- Removed graph `plan_publish`, `filter_plan_by_report`, `create_tags`; moon extension `plan-publish` is now `release`.
- Release decision schema is now 2 (adds `unreleasedVersion`); decisions from earlier builds are rejected.
