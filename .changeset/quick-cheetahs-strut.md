---
callisto-cli: minor
---

**`callisto release` replaces the legacy publish commands**

`callisto release` publishes, tags, and creates GitHub releases for every package whose current version has no tag, from any branch with a clean worktree. `--dry-run` previews the plan anywhere; `--package` restricts it and keeps fixed and linked group members together; an unreleased runtime, optional, or peer dependency must be selected with its dependent; `--receipt <file>` writes the receipt.

Breaking:
- Removed `callisto publish`, `plan-publish`, `tag` (including `--floating-major`), and `filter-plan`. Use `callisto release` or `callisto release --dry-run`.
- Removed `schema --type tag` and `schema --type plan-publish`.
- Removed graph `plan_publish`, `filter_plan_by_report`, `create_tags`, `toposort::publish_order`.
- `release plan --package` now also keeps config-declared linked-group members, and both routes refuse an unselected unreleased dependency (`ReleaseSelectionInvalidReason::DependencyNotSelected`, was `PlatformDependencyNotSelected`).
- Release decisions are written as schema 2 (adds `unreleasedVersion`). Schema 1 files still read; an earlier build cannot read a schema-2 decision.
