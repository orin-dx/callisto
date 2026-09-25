# 1. Changesets-compatible change records

Status: Proposed

## Context

- `@changesets/cli` is JS-only. Polyglot teams already fake `package.json` proxies for Rust and Python packages and copy versions back with a hand-written sync script (docs/00-design.md §0).
- release-please covers many languages but is Conventional-Commits-based; knope and Nx accept either input. None coordinates napi-style platform packages (docs/00-design.md §3.1).
- Callisto's stated edge is teams that prefer the changesets file format over Conventional Commits and need napi-style cross-registry coordination (docs/00-design.md §3.2).

## Decision

The human-written change record is a `.changeset/*.md` file in `@changesets/cli`'s format: YAML frontmatter mapping package names to `none`/`patch`/`minor`/`major`, then a Markdown summary. Compatibility means each tool reads the other's files; output is not byte-identical (callisto quotes names only when needed). Same `pre.json` shape, same `bump_version` semantics (no 0.x remap on an explicit major). Commit inference is opt-in per package (`release-trigger = "auto"`); an unset key resolves to `ReleaseTrigger::Changeset` (`crates/callisto-graph/src/walk.rs`). For an `auto` package, severity is the highest of its changeset entries and its inferred severity (VER-AGG-01, `docs/specs/versioning.json`).

## Options considered

- **Conventional Commits as the primary input** — rejected. That is release-please's model, "the default choice for a team starting from scratch"; callisto does not try to out-build it. The changesets user base "dislikes Conventional-Commits-only tools" (docs/00-design.md §3.2). Inference is kept as an opt-in trigger, and P1's rollback guarantee covers only `Changeset`-trigger packages (§4 P1 scope note).
- **A callisto-specific format** — rejected: adoption and rollback must each be one commit, so existing `.changeset/` files have to keep working in both directions ("One-commit adoption, one-commit rollback. This is the adoption gate; nothing overrides it", docs/00-design.md §4 P1).

## Consequences

- Adopting callisto is meant to be one commit; leaving it is lossless only for `changeset`-trigger packages, since `auto` packages have no changesets equivalent to roll back to (docs/00-design.md §4 P1 scope note). See the gaps below.
- Callisto inherits `@changesets/cli`'s semantics, including the rigid `bump_version` (no 0.x remap on an explicit major; docs/00-design.md §6.2). The opt-in pre-major remap lives in inference only, never in `bump_version` (docs/00-design.md §7.1).
- If `@changesets/cli` ships native polyglot support (its issue #665), callisto's positioning narrows (docs/00-design.md §3.2).
- Today's code falls short of full compatibility: an empty changeset is rejected (E048), a versionless private root `package.json` fails (E013), `pre enter` after `pre exit` is rejected, and a changeset with one unknown entry is consumed (docs/projects/ROAD-TO-V1.md, v1 fix plan §4).

## Enforcement

- Parser and writer tests, including a write-then-parse proptest: `crates/callisto-format/src/changeset/tests.rs`.
- Bump arithmetic tests: `crates/callisto-format/src/bump.rs`.
- No test runs against `@changesets/cli`; compatibility rests on hand-written unit cases.

## Revisit when

- `@changesets/cli` changes its file format or `bump_version` semantics.
- `@changesets/cli` ships native polyglot support (#665).
- The owner decides callisto should target Conventional-Commits-first teams.

## Sources

- docs/00-design.md §0, §3.1, §3.2, §4 (P1), §6.2, §7.1 (deleted in 11038b11b; read with `git show 11038b11b^:docs/00-design.md`)
- PR #143 (bde7305f9): inference always compiled in, gated only by `release-trigger`
- `docs/specs/versioning.json` VER-AGG-01; `crates/callisto-graph/src/aggregate.rs`
- docs/projects/ROAD-TO-V1.md, v1 fix plan §4
