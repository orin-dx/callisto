# 13. An interrupted `version` run resumes from its recorded inputs

Status: Proposed

## Context

- `version` rewrites the files it plans from (manifests, specs, changesets), then refreshes lockfiles. When the refresh fails (E208), a re-run plans from the half-written tree and is refused (E121). The docs name the gap: "a fresh command does not retain that plan" ([versioning](../architecture/versioning.md#idempotent-apply)).
- Release reruns need no recorded state because providers stay observable ([ADR 3](0003-reruns-adopt-landed-effects.md)); a `version` run's inputs are not observable after it has rewritten them.

## Decision

Before its first write, `version` records its inputs (`HEAD`, the working tree, the index, and the tags and flags planning reads) as a commit at `refs/worktree/callisto/checkpoint/version`, and deletes the ref when the run completes. A re-run while the ref exists plans from those inputs and reapplies idempotently. It first applies the plan in a scratch worktree of the checkpoint to learn what a finished run leaves, and refuses, writing nothing, unless every file the plan was derived from or writes is on disk as recorded or as that expected tree has it. The ref is per worktree, so a linked worktree has its own checkpoint. Discarding a run is a command, `callisto version --abort`: it puts back the files the run wrote that still hold its output, removes the files it created (never an ignored one), leaves and reports a file edited since, and keeps the checkpoint until nothing is left to put back.

## Options considered

- **Restore the written files when a step fails** — rejected: each review of #543 found another file state Git already restores correctly (lockfiles from an earlier ecosystem, binary files, symlinks, read-only files).
- **Re-plan from the versions at `HEAD`** — rejected: planning also reads dependency specs from disk, and a prototype resume dropped a dependent the first run had rewritten, exiting 0.
- **Print shell commands that undo the run** — rejected: two reviews found five defects in the printed block (it overwrote edits made since, deleted a user's ignored lockfile, ran from the wrong directory and then dropped the checkpoint, needed quoting). A command decides per file, takes paths as arguments and can stop and be repeated. This departs from ADR 3's "no recovery commands", which concerns release execution, where providers stay observable; a `version` run has no provider to adopt from.
- **Persist the plan itself** — rejected: it must be serialized and versioned and breaks across a Callisto upgrade; recorded inputs can be planned by any version.

## Consequences

- A failed refresh, a crash or Ctrl-C is finished by running `version` again.
- One `version` run at a time per worktree. CI runners never resume; the ref is lost with the runner.
- The ref name and its recorded fields are read across Callisto versions.

## Enforcement

- `crates/callisto-cli/tests/version_resume_tests.rs`: `a_resumed_run_leaves_exactly_what_an_uninterrupted_run_leaves` compares a run whose lockfile refresh fails once and is resumed with one that never failed, across ecosystems, a workspace below the repository root, pre mode and an uncommitted changeset; `changed_inputs_refuse_the_resume_and_the_printed_steps_undo_the_run`; `a_run_that_failed_to_write_its_preview_writes_it_when_run_again`; `a_version_edited_by_hand_after_the_failure_is_refused_not_overwritten`.
- `crates/callisto-model/src/vcs/access.rs`: `a_checkpoint_records_the_tree_leaves_the_user_state_alone_and_is_held_once`.
- VER-APPLY-18 and VER-APPLY-19 in `docs/specs/versioning.json`.

## Revisit when

- Planning can read a workspace from git objects without a worktree.

## Sources

- PR #543 and its review (change #151, trail #357); E121 and E208 in `docs/errors.md`.
- ADR 3 and PR #124.
