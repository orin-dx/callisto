# Plan and apply

How `callisto version` turns changesets into file edits, and where interrupted-command recovery stops. Behavior: [`docs/specs/versioning.json`](../specs/versioning.json). Write mechanics: [ADR 8](../adr/0008-format-preserving-writes-gated-by-apply-permit.md). The emitted decision: [ADR 2](../adr/0002-committed-decision-is-release-authority.md).

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

A crash can stop apply after some files are written. Applying the same in-memory plan again is file-level idempotent; a fresh command does not retain that plan.

- Each `PlannedBump` carries `from` and `to`. Before writing a manifest (or a platform manifest), apply reads its current version: `from` means write, `to` means already done, anything else is `UnexpectedManifestVersion`.
- A manifest already at `to` is still staged: the crashed run may have written it without `git add`.
- Apply refreshes affected lockfiles and stages generated version state before consuming changesets or exiting `pre.json`. It stages input deletions with `git rm --cached --ignore-unmatch` before removing the files from disk, so a failed index update leaves the inputs available for recovery. A fresh command does not retain the in-memory plan and can refuse already-stable staged outputs; E121 directs the user to inspect both diffs and finish consuming retained inputs only when the staged outputs are verified. Automatic replay is not guaranteed.
- `[workspace.package]` version writes (`VersionWriteTarget::CargoWorkspacePackage`) have no `from`/`to` precondition; they are written unconditionally.
- `ApplyOutcome::staged` lists every path apply staged.

## Command boundary and known gaps

`version --emit-decision` derives and validates the decision before writes, applies the plan, and writes the decision only after apply succeeds. The decision file is not wrapped in the stdout report envelope.

Outside `mode: pre`, `check_partial_run` refuses with E121 when a canonical manifest's effective version differs from `HEAD` while a changeset remains in `HEAD` or in the working tree (including untracked files), or when a canonical manifest with an effective staged version change is present in the index. Staged metadata-only edits are allowed. During `mode: exit`, prerelease versions awaiting stable finalization remain eligible, but changed stable versions are not exempt from the retry guard. It does not restore a lost plan. Apply retains changesets and pre-state deletion inputs until lock refresh and staging of generated state succeed, then stages their removal before unlinking them from disk. A late `--emit-decision` write failure is actionable refusal, not automatic replay of an in-memory plan that no longer exists.

Apply refreshes each existing lockfile for every ecosystem represented by an actual version, dependency, platform or optional-dependency write, including dual-manifest packages and workspace-root Cargo writes. Dependency cascade, co-release floor and snapshot rewrites resolve names in each declaration's native manifest context; pnpm workspace protocols and aliases remain distinct inputs. These behaviors are covered by the [versioning contract](../specs/versioning.json) and [findings register](../projects/V1-FINDINGS.md).
