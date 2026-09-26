# 9. A tag's identity is its commit and being annotated

Status: Proposed

## Context

- A rerun adopts effects that already landed (ADR 3), so it must decide whether an existing tag is the one it planned.
- The local check read the tag with `git for-each-ref` and compared the target commit and the annotation text. The remote check uses `git ls-remote`, which returns the tag object and peeled commit but not the annotation text, so it compared only the commit.
- The same repository state could therefore be adopted or rejected depending on whether the remote tag had been fetched.

## Decision

An existing tag is the planned tag when it names the planned commit and is annotated. Annotation text is not compared, locally or remotely. A lightweight tag or a tag on another commit is a conflict (E167) and is never adopted.

## Options considered

- **Compare the annotation text on both sides** — rejected: the remote side needs the tag object fetched first, and a tag created by a person or an earlier template would block every rerun.
- **Keep the split (text locally, commit remotely)** — rejected: the outcome depends on fetch state.

## Consequences

- Local and remote checks agree.
- A tag with a different message is adopted. The receipt records only the peeled commit (`ProviderEvidenceV1::GitTag { peeled_commit }` in `crates/callisto-model/src/release_observation.rs`), and `gh release create --verify-tag` checks only that the tag exists.

## Enforcement

- Tests in `crates/callisto-graph/src/commands/release/provider/tag.rs` for local and remote observation (different annotation text is adopted; lightweight and other-commit tags conflict).
- REL-RERUN-02 and REL-RERUN-03 in `docs/specs/release.json`.

## Revisit when

- Something downstream starts reading the annotation text (release notes, signing policy, provenance).

## Sources

- Owner decision, `docs/projects/ROAD-TO-V1.md` "v1 fix plan" decisions (2026-09-25)
- The fix in this PR: "fix: stop comparing tag annotation text in release rerun observation"
