---
callisto-conventional: minor
callisto-vcs: patch
callisto-graph: patch
callisto-model: patch
callisto-format: patch
callisto-manifests: patch
---

# Git-access layer decoupling, performance, and small correctness fixes

## Breaking Changes

- **`callisto-conventional` no longer depends on `callisto-vcs`.** `fetch_commits`/`infer_severity` now take `&dyn callisto_model::CommitWalker`; `ConventionalError::Vcs` renamed to `ConventionalError::CommitWalk`.

## Fixes

- `BREAKING CHANGE:` commit-footer parsing fixed (`callisto-conventional`).
- Merged-branch commits crossing the `since` boundary are no longer dropped from `commits_since_with_pathspec` (`callisto-vcs`).
- Caret-range coverage (`caret_covers`) now correctly excludes pre-release versions — `^1.2.3` no longer incorrectly covers `1.9.0-alpha.1` (`callisto-graph`).
- Cargo compound dependency-range rewriting now stays within the upper bound (`callisto-manifests`).
- PEP 440 `is_prerelease`/`VersionReq` normalization fixed (`callisto-model`).
- Python dependency-name matching now applies PEP 503 normalization (`callisto-model`, `callisto-manifests`, `callisto-graph`).
- npm workspace package-manager (pnpm/yarn/bun/npm) is now auto-detected for publish (`callisto-graph`).
- Several duplicate diagnostic codes — different errors that previously shared one code — now have their own (`callisto-model`, `callisto-format`, `callisto-graph`).

## Performance

- `status`, `plan_snapshot`, `plan_publish`, and `plan_version` now share one `GitAccess` instead of rediscovering the repository per package or command (`callisto-graph`).
- Tag existence checks now use the cached `TagIndex` instead of a lookup per release (`callisto-graph`).
