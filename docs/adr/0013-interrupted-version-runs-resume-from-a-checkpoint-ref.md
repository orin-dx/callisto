# 13. Interrupted `version` runs resume from a checkpoint ref

Status: Proposed

## Context

- `callisto version` writes manifests, changelogs and `pre.json`, refreshes lockfiles with each ecosystem's tool, stages, then consumes changesets. Inputs are kept until refresh and staging succeed ([versioning](../architecture/versioning.md#idempotent-apply)).
- After a failed refresh (E208) the advised re-run was refused with E121, because the run had already changed the versions it plans from. The architecture names the gap: "a fresh command does not retain that plan… Automatic replay is not guaranteed."
- A release's landed effects stay observable at the providers, so a re-run can adopt them ([ADR 3](0003-reruns-adopt-landed-effects.md)). A `version` run is different: it rewrites the very files it plans from (manifests, specs, changesets), so a re-run cannot observe what the interrupted run planned.

## Decision

Persist the inputs of an interrupted `version` run, never its progress, and re-derive the work from them.

- **Checkpoint.** Before its first write, `version` records `HEAD`, the working tree and the index as a commit at `refs/callisto/checkpoint/version`, created only if absent and never signed. The commit also records what planning reads beyond files: `HEAD`, the tag refs it uses, the plan-changing flags and the Callisto version. It covers the workspace and its release inputs, not the whole repository.
- **Complete.** The ref is deleted after the last effect, including the `--emit-decision` and `--emit-preview` files.
- **Resume.** A plain re-run while the ref exists plans from a worktree of the checkpoint and applies that plan to the real tree. The existing `from`/`to` guard and the changelog's duplicate-heading check make the writes idempotent. There is no recovery command, as under ADR 3.
- **Refuse.** If a recorded input changed, or a manifest is at neither `from` nor `to`, nothing is written and the user is shown how to abort: restore the tree and index from the checkpoint and remove the files the run created.
- E121 stays for states Callisto did not record.

Architecture:

- The checkpoint primitives live in `GitAccess` and run the system `git` ([ADR 5](0005-system-git-only.md)); creating one is a write and needs an `ApplyPermit` ([ADR 8](0008-format-preserving-writes-gated-by-apply-permit.md)).
- The command layer owns the lifecycle. Planning is unchanged: a resume loads the workspace from the checkpoint's worktree with the existing loader, so ecosystems need nothing new.
- Like the committed decision for a release ([ADR 2](0002-committed-decision-is-release-authority.md)), the checkpoint is the authority for what the interrupted run was doing; the `from`/`to` guard is its adoption. Because every resume re-observes the tree, recorded state cannot disagree with the files the way the release state file did (#124).

Scope: `version` only. `pre enter` and `pre exit` write one file and become idempotent instead. `release` keeps ADR 3. `init` fails only before writing. `snapshot` may adopt the checkpoint to undo its writes later, under this ADR.

## Options considered

- **Restore the written files in process on failure** — rejected. Each review of #543 found another case Git already handles (a lockfile an earlier ecosystem refreshed, binary lockfiles, symlinks, read-only files).
- **Report E208 after consuming the changesets** — rejected. It consumes the inputs before the refresh succeeds, the recovery material the apply ordering keeps.
- **Check in core that every spec covers the new versions** — rejected. Which specs must resolve is each ecosystem's rule, and [ADR 10](0010-native-resolution-after-version.md) makes the native tool the authority.
- **Re-plan from `HEAD` versions** — rejected. Planning also reads specs from disk; a prototype dropped a dependent the interrupted run had already rewritten, and exited 0.
- **A plan journal file** — rejected. It needs the plan serialized and versioned, and breaks across a Callisto upgrade; inputs can be planned from by any version.
- **Rehearse every run in a scratch copy** — rejected. It pays a copy and a second refresh on every run, not only after a failure.
- **`git stash`, a branch, or git notes** — rejected. The stash stack is the user's, a branch is published by `push --all`, notes need a commit that does not exist yet.

## Consequences

- A failed refresh, a crash or Ctrl-C mid-apply is finished by running `version` again.
- Only one `version` run can be in flight per repository.
- The checkpoint is hidden from `git branch`, `git tag`, `git stash list`, `push --all` and `--tags`; `push --mirror` copies it and `git log --all` shows it.
- CI never resumes, because each runner is fresh; the ref goes with the runner.
- The ref name and its recorded metadata are read across Callisto versions and belong in [wire-compat](../architecture/wire-compat.md).

## Enforcement

- A differential test: a run whose lockfile refresh fails once and is resumed leaves the same files, index and status as a run that never failed, across ecosystems and workspace layouts.
- Tests that a changed input, a hand-edited version and a concurrent run are refused, and that the abort steps restore the state before the run.

## Revisit when

- Planning can read a workspace from git objects, removing the worktree.
- Another local multi-step command needs resume or abort.

## Sources

- PR #543 and its review findings (change #151, trail #357); the E121 and E208 rows in `docs/errors.md`.
- ADR 3 and PR #124 for why release execution keeps no state.
- Prototype runs, 2026-11-14, to be reproduced by the enforcement tests.
