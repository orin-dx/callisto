# 13. An interrupted `version` run resumes from its recorded inputs

Status: Proposed

## Context

- `version` rewrites the files it plans from (manifests, specs, changesets), then refreshes lockfiles. When the refresh fails (E208), a re-run plans from the half-written tree and is refused (E121). The docs name the gap: "a fresh command does not retain that plan" ([versioning](../architecture/versioning.md#idempotent-apply)).
- Release reruns need no recorded state because providers stay observable ([ADR 3](0003-reruns-adopt-landed-effects.md)); a `version` run's inputs are not observable after it has rewritten them.

## Decision

Before its first write, `version` records its inputs (`HEAD`, the working tree, the index, and the tags and flags planning reads) as a commit at `refs/callisto/checkpoint/version`, and deletes the ref when the run completes. A re-run while the ref exists plans from those inputs and reapplies idempotently; if an input changed since, it refuses and writes nothing.

## Options considered

- **Restore the written files when a step fails** — rejected: each review of #543 found another file state Git already restores correctly (lockfiles from an earlier ecosystem, binary files, symlinks, read-only files).
- **Re-plan from the versions at `HEAD`** — rejected: planning also reads dependency specs from disk, and a prototype resume dropped a dependent the first run had rewritten, exiting 0.
- **Persist the plan itself** — rejected: it must be serialized and versioned and breaks across a Callisto upgrade; recorded inputs can be planned by any version.

## Consequences

- A failed refresh, a crash or Ctrl-C is finished by running `version` again.
- One `version` run at a time per repository. CI runners never resume; the ref is lost with the runner.
- The ref name and its recorded fields are read across Callisto versions.

## Enforcement

To be filled by the implementing PR: the differential resume test and the refusal tests.

## Revisit when

- Planning can read a workspace from git objects without a worktree.

## Sources

- PR #543 and its review (change #151, trail #357); E121 and E208 in `docs/errors.md`.
- ADR 3 and PR #124.
