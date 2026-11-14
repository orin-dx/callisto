# 13. Interrupted `version` runs resume from a checkpoint ref

Status: Proposed

## Context

- `callisto version` writes manifests, changelogs and `pre.json`, refreshes lockfiles with each ecosystem's tool, stages, then consumes changesets ([versioning](../architecture/versioning.md#idempotent-apply)). Inputs are kept until refresh and staging succeed, so a failure leaves them available.
- A refresh failure (E208) told the user to fix the cause and re-run, but the re-run was refused with E121, because the run had already changed the versions it plans from. The docs name the gap: "a fresh command does not retain that plan… Automatic replay is not guaranteed." Two bug hunts found it independently after 0.20.0.
- Unlike a release, whose landed effects stay observable at the providers ([ADR 3](0003-reruns-adopt-landed-effects.md)), a `version` run destroys its own inputs: the manifests, specs, config and changesets it plans from are the files it rewrites. A re-run cannot observe what the interrupted run planned from.
- A prototype that re-planned from `HEAD` versions was wrong: dependency specs on disk had already been rewritten, so the cascade dropped a dependent and the resume exited 0 with that dependent's manifest unstaged.

## Decision

Before its first write, `version` records its inputs in a checkpoint: a commit of `HEAD` plus the working tree and the index as they are, built in a temporary index (`GIT_INDEX_FILE`), never signed, and pointed to by `refs/callisto/checkpoint/version`. The ref is created only if absent. The checkpoint commit also records what the plan depends on beyond files: `HEAD`, the tag refs the plan reads, the plan-changing flags and the Callisto version.

- **Complete.** The ref is deleted after the last effect, including the `--emit-decision` and `--emit-preview` files.
- **Resume.** Running `version` again while the ref exists re-plans from a throwaway worktree of the checkpoint and applies that plan to the real tree. The existing `from`/`to` guard and the changelog's duplicate-heading check make the writes idempotent, and the refresh, staging and consumption run again. There is no recovery command: a plain re-run resumes, as a plain re-run finishes a release under ADR 3.
- **Refuse.** If any recorded input changed since the checkpoint (config, changesets, `pre.json`, `HEAD`, the recorded tags, the flags) or a manifest is at neither `from` nor `to`, the resume stops with nothing written and prints the exact git commands to abort.
- **Abort.** Restore the working tree and index from the checkpoint and remove the files the run created; Git restores symlinks, binary files and modes.
- E121 stays for states Callisto did not record (hand edits, older Callisto versions).

The checkpoint holds inputs, not progress: every resume re-observes the tree through the `from`/`to` guard, so no recorded state can disagree with the files the way the release state file did (#124).

## Options considered

- **Restore the written files in process on failure** — rejected. Implemented in #543: four review rounds each found a new case (a lockfile an earlier ecosystem refreshed, binary lockfiles, symlinks, read-only files). It reimplements a filesystem undo that Git already has.
- **Refresh lockfiles after consuming the changesets, and report E208 as a post-condition** — rejected. It consumes the inputs before the refresh succeeds, the recovery material the apply ordering protects.
- **Check in core, before writing, that every spec covers the new versions** — rejected as the fix. Which specs must resolve is each ecosystem's rule (Cargo needs a path dev-dependency's version to match; others differ), and [ADR 10](0010-native-resolution-after-version.md) makes the native tool the authority. It may return later as a hint inside one ecosystem.
- **Re-plan from `HEAD` versions** — rejected. Prototyped: the plan also reads specs from disk, so a resumed run dropped a dependent the first run had planned.
- **A plan journal file in `.git`** — rejected. It needs the plan serialized and versioned, and breaks when Callisto is upgraded between the runs; the checkpoint stores inputs, which any version can plan from.
- **Rehearse every run in a scratch copy first** — rejected for now. It also avoids the half-applied state, but costs a copy and a second refresh on every run instead of only after a failure.
- **`git stash`, a branch, or git notes as storage** — rejected. The stash stack is the user's and has no namespace; a branch is visible and `push --all` publishes it; notes attach to commits that do not exist yet.

## Consequences

- E208's advice ("fix it, then re-run `callisto version`") becomes true, and a crash or Ctrl-C mid-apply resumes the same way.
- A checkpoint is invisible to `git branch`, `git tag`, `git stash list`, and to `push --all` and `--tags`; `push --mirror` copies it and `git log --all` shows it. `git gc` keeps its objects until the ref is deleted.
- Each run costs a few git processes for the checkpoint: about 80 ms for this repository. It is limited to the workspace and its release inputs, because one large untracked file made the whole-tree version take 3.2 s.
- A run in CI never resumes (every runner is fresh); the ref is deleted on success and discarded with the runner on failure.
- Only one `version` run can be in flight per repository.
- The ref name and the checkpoint metadata are read across Callisto versions, so they are listed in [wire-compat](../architecture/wire-compat.md).
- `snapshot` can use the same checkpoint to undo its transient writes; that is a follow-up, not part of this decision.

## Enforcement

- A differential test: for each workspace shape, a run whose lockfile refresh fails once and is then resumed leaves the same files, index and status as a run that never failed. Shapes: Cargo cascade with an existing changelog, fixed group with inherited versions, Cargo and npm where npm fails after Cargo refreshed, Python with `uv.lock`, a workspace below the repository root, pre mode with a group, an uncommitted changeset, a symlinked lockfile.
- Refusal tests: a hand-edited version, an edited `callisto.toml`, a tag created on `HEAD` between the runs, and a second concurrent run.
- An abort test: the printed commands return the tree, index and status to their state before the run.

## Revisit when

- Planning can run from git objects without a worktree, which would remove the throwaway checkout.
- Another local multi-step command needs resume or abort.
- A Git version changes how `commit-tree`, `update-ref` or a temporary index behave.

## Sources

- Bug hunts after 0.20.0: two independent reports of the E208 retry refused by E121.
- PR #543 and its review findings (changes #151 and #153 on trail #357).
- Local prototype and differential runs, 2026-11-14 (not committed).
- ADR 3 and PR #124 for why release execution keeps no state; `docs/architecture/versioning.md` "Idempotent apply".
