# 11. Packages are keyed internally by directory, not by their primary name

Status: Proposed

## Context

- Before this PR, a package's `PackageId` (its primary manifest's name, promoted to `eco/name` only on
  a name collision) was the only key used to look it up, store it and compare it.
- A dual-manifest package (e.g. a Cargo crate with a co-located `package.json`) could only be selected,
  grouped or matched by its primary manifest's name; the same package's other manifest name resolved to
  nothing.
- A `[[package]]` or `[[fixed-group]]` rule matched by exact `PackageId`, so a rename that promoted one
  package to `eco/name` changed every existing selector, config rule and JSON output field naming it,
  even when nothing about the package itself changed -- only another ecosystem gaining a same-named
  package did.

## Decision

A package is keyed internally by the directory its manifests live in. Each directory's package carries
one or more `(ecosystem, native manifest name)` names, one per admitted manifest. `IdentityIndex` (in
`crates/callisto-graph/src/identity.rs`) maps every `(ecosystem, name)` pair to the directory's identity,
so any of a package's names resolves to the same package.

Promotion to a displayed `eco/name` id happens only when two directories declare the same name in
different ecosystems; it is a display transform over the directory-keyed identity, not the key itself.

## Options considered

- **The primary manifest name as the key** (previous behavior) -- rejected: it changed an existing
  package's id and JSON output whenever another ecosystem later added a package with the same name, and
  made a dual-manifest package's non-primary name permanently unresolvable.

## Consequences

- Selectors, config rules and JSON output stay stable under EITHER the addition of a same-named package
  in another ecosystem elsewhere in the workspace, or a change to which manifest ecosystem is "primary".
- A dual-manifest package resolves through any of its manifests' native names, not only the primary one.
- Every consumer of package identity (`--package`, `add`, `[[package]]`, `[[package-set]]`,
  `[[fixed-group]]`/`[[linked-group]]`, product-package, artifacts, matrix) goes through one
  `IdentityIndex::resolve` instead of comparing `PackageId` values directly.

## Enforcement

- `crates/callisto-model/src/package.rs`: `Package` is one per directory; `manifests: Vec<ManifestDecl>`
  holds every admitted manifest for that directory, each with its own ecosystem and native name.
- `crates/callisto-graph/src/identity.rs`: `IdentityIndex::{bare, prefixed, native}` map every
  registered name to the owning directory's `PackageId`; `resolve`/`resolve_id` is the one selector
  resolver every caller uses.
- `crates/callisto-graph/src/walk.rs`: manifest discovery groups manifests by directory before assigning
  identity, and raises E100 only when two directories share a name within the same ecosystem.
- WS-ID-01 through WS-ID-07 in `docs/specs/workspace.json`.

## Revisit when

- A package needs to move directories without losing its identity (e.g. a repo reorganization); today's
  key is the directory path, so a move is a new package unless something outside this ADR's scope
  reconciles it.

## Sources

- Owner decision, `docs/projects/ROAD-TO-V1.md` "v1 fix plan" decisions (2026-09-25): "Packages are keyed
  internally by directory; promotion to `eco/name` is display-only."
- This PR: `IdentityIndex` and the directory-grouped `Package`/`ManifestDecl` model.
