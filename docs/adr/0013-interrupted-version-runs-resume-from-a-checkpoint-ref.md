# 13. An interrupted `version` run resumes from its recorded inputs

Status: Proposed

## Context

- `version` rewrites the files it plans from (manifests, specs, changesets), then refreshes lockfiles. When the refresh fails (E208), a re-run plans from the half-written tree and is refused (E121). The docs name the gap: "a fresh command does not retain that plan" ([versioning](../architecture/versioning.md#idempotent-apply)).
- Release reruns need no recorded state because providers stay observable ([ADR 3](0003-reruns-adopt-landed-effects.md)); a `version` run's inputs are not observable after it has rewritten them.

## Decision

Before its first write, `version` records its inputs (`HEAD`, the working tree, the index, and the tags and flags planning reads) as a commit at `refs/worktree/callisto/checkpoint/version`, and deletes the ref when the run completes. A re-run while the ref exists plans from those inputs and reapplies idempotently. It first applies the plan in a scratch worktree of the checkpoint to learn what a finished run leaves, and refuses, writing nothing, unless every file the plan was derived from or writes is on disk as recorded or as that expected tree has it. The ref is per worktree, so a linked worktree has its own checkpoint, and it records the workspace so another workspace of the worktree does not resume it. A refused resume has one way out: delete the ref (`git update-ref -d …`, named in the error) and follow the E121 steps, which is today's behaviour.

## Options considered

- **Restore the written files when a step fails** — rejected: each review of #543 found another file state Git already restores correctly (lockfiles from an earlier ecosystem, binary files, symlinks, read-only files).
- **Re-plan from the versions at `HEAD`** — rejected: planning also reads dependency specs from disk, and a prototype resume dropped a dependent the first run had rewritten, exiting 0.
- **An undo for a refused resume** (printed shell commands, then a `version --abort` command) — rejected for now: three review rounds found about a dozen defects in it (edits made since overwritten, an ignored lockfile deleted, wrong working directory, staged and untracked files, Git versions before `git restore`), and nothing shows users need more than deleting the ref and following E121. This matches the 2026-10-30 decision to defer recovery beyond resume. Revisit if refusals prove common.
- **Persist the plan itself** — rejected: it must be serialized and versioned and breaks across a Callisto upgrade; recorded inputs can be planned by any version.

## Consequences

- A failed refresh, a crash or Ctrl-C is finished by running `version` again.
- One `version` run at a time per worktree. CI runners never resume; the ref is lost with the runner.
- The ref name and its recorded fields are read across Callisto versions.

## Enforcement

- `crates/callisto-cli/tests/version_resume_tests.rs`: `a_resumed_run_leaves_exactly_what_an_uninterrupted_run_leaves` compares a run whose lockfile refresh fails once and is resumed with one that never failed, across ecosystems, a workspace below the repository root, pre mode and an uncommitted changeset; `a_run_that_failed_to_write_its_preview_writes_it_when_run_again`; the refusal tests (`a_manifest_edited_after_the_failure_refuses_the_resume`, `a_dependency_requirement_edited_after_the_failure_refuses_the_resume`, `a_changeset_deleted_by_hand_after_the_failure_refuses_the_resume`, `a_different_workspace_does_not_resume_anothers_checkpoint`); and `a_refused_resume_can_be_given_up_by_deleting_the_checkpoint`.
- `crates/callisto-model/src/vcs/access.rs`: `a_checkpoint_records_the_tree_leaves_the_user_state_alone_and_is_held_once`.
- VER-APPLY-19 and VER-APPLY-20 in `docs/specs/versioning.json`.

## Revisit when

- Planning can read a workspace from git objects without a worktree.

## Sources

- PR #543 and its review (change #151, trail #357); E121 and E208 in `docs/errors.md`.
- ADR 3 and PR #124.
