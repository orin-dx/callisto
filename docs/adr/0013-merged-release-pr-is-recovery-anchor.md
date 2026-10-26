# 13. The merged release PR anchors recovery

Status: Accepted (implemented in #383)

## Context

- A failed release may remain unfinished after main advances. A local retry from the new tip no longer selects packages tagged on the older commit.
- The merged release commit already holds the committed version decision. GitHub associates that commit with the managed PR.
- A proposed version tag would need to move when the PR changes and could remain stale after a failed cleanup.

## Decision

Callisto's own GitHub workflow labels its managed release PR pending before merge. Each main run selects the oldest merged managed PR still pending and uses its merge commit as the release source with current orchestration. A full SHA dispatch remains available for PRs without the marker. The workflow changes the marker to complete only after execution and receipt upload succeed, then dispatches the next pending release or refreshes the release PR for newer changesets. The marker is an advisory queue: the committed decision and direct provider observations still determine what may be published and what has finished.

## Options considered

- **Movable proposed version tag** — rejected because branch updates require moving it, a stale tag can look authoritative, and cleanup adds a separate failure point.
- **Last published tag as the recovery source** — rejected because a partial release may have no tag, and a tag does not describe the full committed release roster.
- **Only manual SHA dispatch** — retained as a fallback, but it does not automatically drain pending releases when main advances.

## Consequences

- Merged PRs created before the pending label was introduced need explicit SHA dispatch if publication fails.
- Removing or altering labels can change queue discovery, but cannot change the committed release decision or bypass source verification.
- The workflow needs permission to edit PR labels and dispatch the next run; only execution receives registry credentials.
- Generated consumer workflows do not opt into the pending marker until they support the same recovery route.

## Enforcement

- `.github/tests/test_pending_release_candidate.sh` and `.github/tests/test_mark_release_complete.sh` exercise queue selection and completion marking.
- `.github/tests/verify-release-workflow-policy.sh` checks scheduling and job permissions.
- The recovery criteria in `docs/specs/release.json` describe the workflow behavior.

## Revisit when

- Generated workflows implement committed-decision recovery or a forge other than GitHub requires another queue mechanism.

## Sources

- Owner discussion of moved-main recovery, release PR merge SHA, and release-please-style pending state (2026-10-25).
- ADR 2: the committed version decision is the release authority.
- ADR 3: reruns observe providers and adopt landed effects without persisted execution state.
