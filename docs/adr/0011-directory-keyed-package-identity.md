# 11. Packages are keyed internally by directory; eco/name is display-only

Status: Proposed

## Context

- `PackageId` today doubles as both the graph's internal key and the grammar a user or config file types (`--package cargo/foo`, `[[package]] match = "..."`). `PackageId::matches` treats a `Bare` id as an ecosystem wildcard, so a query for one ecosystem can silently select a same-named package in another (`--package cargo/foo` selecting an npm-only `foo`; a `pypi:foo` `[[package]]` rule applying to a Cargo-only `foo`).
- A dual-manifest package's id comes from its highest-priority ecosystem's manifest (Cargo, then npm, then pyproject); the same package cannot be selected by its other manifest's own name (`add --package @x/core` fails when the package's id is its Cargo name).
- Every place that resolves a user- or config-typed selector against the graph re-implements some version of "match by name, then disambiguate by ecosystem," with several call sites carrying a hand-rolled `selector.matches(&candidate) && selector.ecosystem() == Some(candidate_ecosystem)` guard as a workaround.

## Decision

Packages are keyed internally by their directory and carry one or more (ecosystem, native name) names, recorded once during workspace discovery (`IdentityIndex.native`). Promotion to a displayed `eco/name` id is a display concern, not an identity concern. This PR lands the selector-resolution half of that shape (below); replacing `PackageId` itself with a directory key is tracked as follow-up work this ADR authorizes.

Every selector a user or config file types -- `--package`, `add`, `[[package]]`, `[[package-set]]`, `[[fixed-group]]`/`[[linked-group]]` members, `product-package`, `[[release.artifact]]` -- resolves through one function, `IdentityIndex::resolve`/`resolve_id`: a qualified selector (`eco:name` or `eco/name`) is an exact lookup by registered native name; a bare selector searches every registered native name and requires exactly one distinct package. A miss is E102; more than one distinct match is E103. This is the one place ecosystem-exactness and any-native-name resolution are implemented; call sites stop hand-rolling it.

## Options considered

- **The primary manifest name as the key** (today's shape) -- rejected: it changes an existing package's id and JSON output whenever another ecosystem later adds a package sharing that name (promotion), and makes every non-primary manifest name unresolvable by any selector.
- **Keep `PackageId::matches`'s ecosystem-wildcard semantics for selector resolution too** -- rejected: this is the source of the qualified-selector and `[[package]]`-rule cross-ecosystem bugs; a bare id is not evidence a package lacks an ecosystem, only that its selector didn't state one.

## Consequences

- A package's id and JSON output no longer change when an unrelated directory in another ecosystem starts using the same name.
- A dual-manifest package is nameable by any of its manifests' own names, not only its primary one.
- Every selector surface shares one ambiguity rule (E102/E103) instead of each call site's own approximation.

## Enforcement

- `crates/callisto-graph/src/identity.rs`: `IdentityIndex::resolve`, `resolve_id`, `identifies`.
- `crates/callisto-graph/src/config/resolve.rs`: `[[package]]` rule matching (`rule_applies`) is ecosystem-exact against a package's real manifests, not `PackageId::matches`.
- `crates/callisto-graph/src/config/pattern.rs`: `[[package-set]]` accepts `eco:glob` and `eco/glob` as the same grammar.
- `crates/callisto-graph/src/config/groups.rs`, `commands/matrix.rs`, `commands/release_decision.rs`, `commands/release/derive.rs`, `crates/callisto-cli/src/commands/add.rs`: resolve every selector through `IdentityIndex`, not `PackageId::matches`.
- WS-ID-02, WS-ID-05, WS-ID-06, WS-CFG-01, WS-CFG-04 in `docs/specs/workspace.json`.

## Revisit when

- A follow-up PR replaces `PackageId`'s Bare/Prefixed representation with a real directory key; until then `PackageId` stays as is and `IdentityIndex` is the layer that already behaves as if it were directory-keyed.

## Sources

- Owner decision, `docs/projects/ROAD-TO-V1.md` "v1 fix plan" decisions (2026-09-25), section "3. Every package resolves by any of its names"
- The fix in this PR: `IdentityIndex::resolve`/`resolve_id`/`identifies` and their call sites
