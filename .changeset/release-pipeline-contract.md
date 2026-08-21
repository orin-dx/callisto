---
callisto-model: minor
callisto-graph: minor
callisto-cli: minor
callisto-moon: patch
---

# Release-pipeline / CI Action contract correctness

## Breaking Changes

- **JSON report fields renamed** — visible via `callisto-cli --format json`:
  - `ValidateReport.valid` → `ok`
  - `ComposePrBodyReport.pr_body` → `body`
  - `TagReport.created_tags` → `tags` (plus new `already_existed: bool`)
  - `StatusReport` gained `hasChangesets`, `lastReleasedVersion`, `releaseTrigger`
  - `ReleaseEntry` gained a required `isPrerelease: bool`, computed from the resolved version (covers both SemVer's `-` prerelease component and PEP 440's dev/pre segments)
  - Update anything parsing this JSON, including the shipped GitHub Action.

## Fixes

- `callisto tag` no longer fabricates a `sha`: when a tag already exists at a different commit, it now resolves the real target via `git rev-parse` (both apply and `--dry-run` modes).
- The shipped GitHub Action's `status --check` branching now matches the CLI's real exit codes (1 = pending, 2 = none pending) — the release-PR creation step was previously unreachable.
- The GitHub Action's GitHub Release creation loop now reads each release entry's `isPrerelease` field directly instead of guessing from the tag string (the old `-(alpha|beta|rc|pre|next)` regex missed PEP 440 prereleases like `1.2.3a1` entirely), and hard-fails before creating a release if that field is missing or non-boolean rather than silently treating it as `false`.
- `plan_publish` now reads the real `CHANGELOG.md` section back for each release instead of always leaving `changelogSection` blank.
