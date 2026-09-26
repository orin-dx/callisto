# Plan and apply

How `callisto version` turns changesets into file edits, and why a crashed run can be re-applied. Behavior: [`docs/specs/versioning.json`](../specs/versioning.json). Write mechanics: [ADR 8](../adr/0008-format-preserving-writes-gated-by-apply-permit.md). The emitted decision: [ADR 2](../adr/0002-committed-decision-is-release-authority.md).

## Two phases

| Phase | Entry point | Touches disk |
| --- | --- | --- |
| Plan | `commands::plan_version` (`crates/callisto-graph/src/commands/version.rs`) returns a `VersionPlan` | No |
| Apply | `apply_version_plan` (`crates/callisto-graph/src/apply.rs`) | Yes; takes `&ApplyPermit` |

- `VersionPlan` (`crates/callisto-graph/src/plan.rs`) is the whole change: bumps, dependency spec rewrites, platform manifest writes, owner `optionalDependencies` pins, changelog sections, consumed changesets, `pre.json` updates and diagnostics. It is in memory only (no `Serialize`).
- `plan_version` is shared: `version`, `status`, `compose-pr-body` and `release plan --package` all compute the same plan.
- A dry run never gets a permit, so it reports the plan and never calls apply.
- `snapshot` applies with `ApplyOptions::transient`: manifests are written, while changelogs, changeset deletion and git staging are skipped.

## Idempotent apply

A crash can stop apply after some files are written. Re-running the same plan must finish the job, not fail or double-bump.

- Each `PlannedBump` carries `from` and `to`. Before writing a manifest (or a platform manifest), apply reads its current version: `from` means write, `to` means already done, anything else is `UnexpectedManifestVersion`.
- A manifest already at `to` is still staged: the crashed run may have written it without `git add`.
- Every consumed changeset path is staged whether or not the file still exists; deletions go through `git rm --cached --ignore-unmatch`, so a file deleted before the crash is still removed from the index.
- `[workspace.package]` version writes (`VersionWriteTarget::CargoWorkspacePackage`) have no `from`/`to` precondition; they are written unconditionally.
- `ApplyOutcome::staged` lists every path apply staged.
