# 1. Changesets-compatible change records

Status: Accepted

## Context

- `@changesets/cli` is JS-only. Polyglot teams already fake `package.json` proxies for Rust and Python packages and hand-sync versions back (docs/00-design.md §0).
- release-please covers many languages but is Conventional-Commits-based; knope and Nx accept either input. None coordinates napi-style platform packages (docs/00-design.md §3.1).
- Callisto's stated edge is teams that prefer the changesets file format over Conventional Commits and need cross-registry coordination (docs/00-design.md §3.2).

## Decision

The human-written change record is a `.changeset/*.md` file byte-compatible with `@changesets/cli`: same frontmatter shape, same `pre.json`, same `bump_version` semantics (no 0.x remap on an explicit major). Commit inference exists but is opt-in per package (`release-trigger = "auto"`; `ReleaseTrigger::Changeset` is the default in `crates/callisto-model/src/ecosystem.rs`). A changeset always wins over inference.

## Options considered

- **Conventional Commits as the primary input** — rejected. That is release-please's model, "the default choice for a team starting from scratch"; callisto does not try to out-build it. The changesets user base "dislikes Conventional-Commits-only tools" (docs/00-design.md §3.2). Inference is kept as an opt-in trigger, and P1's rollback guarantee covers only `Changeset`-trigger packages (§4 P1 scope note).
- **A callisto-specific format** — rejected. P1: "Byte-compatibility with `@changesets/cli`'s file format is a hard requirement … One-commit adoption, one-commit rollback. This is the adoption gate; nothing overrides it" (docs/00-design.md §4).
- **Reuse the `knope-dev/changesets` crate for parsing** — rejected: it splits `name: severity` before unquoting and mangles quoted `@scope/name` entries (docs/00-design.md §6.1).

## Consequences

- Adopting or leaving callisto is one commit; existing `.changeset/` directories work unchanged.
- Callisto inherits `@changesets/cli`'s semantics, including ones it might otherwise change (the rigid `bump_version`). The opt-in pre-major remap lives in inference only, never in `bump_version` (docs/00-design.md §7.1).
- If `@changesets/cli` ships native polyglot support (its issue #665), callisto's positioning narrows (docs/00-design.md §3.2).

## Enforcement

- Parser and writer tests, including a write-then-parse proptest: `crates/callisto-format/src/changeset/tests.rs`.
- Bump arithmetic tests: `crates/callisto-format/src/bump.rs`.
- No test runs against `@changesets/cli` itself; compatibility is asserted by fixtures written to its documented behaviour.
- `ReleaseTrigger::Changeset` is the `#[default]` variant.

## Revisit when

- `@changesets/cli` changes its file format or `bump_version` semantics.
- `@changesets/cli` ships native polyglot support (#665), making a separate reader redundant.
- The owner decides callisto should target Conventional-Commits-first teams.

## Sources

- docs/00-design.md §0, §3.1, §3.2, §4 (P1), §6.1, §7.1 (deleted in 11038b11b; read with `git show 11038b11b^:docs/00-design.md`)
- PR #143 (bde7305f9): inference always compiled in, gated only by `release-trigger`
