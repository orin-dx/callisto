# 12. Canonical distribution references and explicit release ownership

Status: Proposed

This proposal revises ADR 11's selector promotion and default-tag coupling when implemented. ADR 11 remains the current decision; it is not superseded by this documentation change.

## Context

The owner selected Rust/Cargo, npm/pnpm and Python/PyPI as the current scope, with architectural room for Go and .NET/NuGet. The system audit found that display promotion leaks into dependency rewrites, historical ownership and shared tag/forge ordering. Fresh Cargo/Maturin projects also fail because one package requires one grammar. Evidence and implementation work are recorded in [the findings register](../projects/V1-FINDINGS.md).

## Proposed decision

- Use canonical slash-qualified distribution references for new exact inputs, authored and generated data; reject new bare and colon-form aliases. Native normalization belongs to the ecosystem.
- Separate release ownership, distribution identity, authoritative version source and external effects. Reuse existing types and graphs; do not add a general plugin framework or opaque IDs without need.
- Read existing bare/colon references only through a deliberate compatibility/migration boundary with established ownership. Preserve historical tag ownership without using display promotion as a release-identity change.
- Keep native names and manifest context for dependency edits, inherited/derived versions and publisher arguments.
- Within Callisto's release-operation graph, shared external effects have one owner and wait for every required publication/output of that owner. For current ecosystems, public announcement follows registry completion; a future Go source tag can itself be publication.
- Native package-manager lifecycle hooks retain package-owner-defined behavior and environment. Callisto does not inspect, rewrite, suppress or reject hooks; effects initiated by owner-defined hooks are outside Callisto's release-operation graph.
- Reruns observe every required effect, including unfinished work after tag creation and missing Python files. Keep ADR 2's committed roster and ADR 3's no-journal recovery.

The exact owner representation, legacy-record migration, wire schema migration and channel/private-registry policies must be resolved in the relevant implementation change. This ADR does not approve those unspecified details or introduce release gates.

## Options considered

- Keep collision-triggered bare/qualified identity everywhere: retains inference rules at every new consumer and lets unrelated workspace changes alter identity behavior.
- Canonicalize one package ID but retain one grammar and per-distribution completion: does not represent Maturin version authority or shared tag/forge effects.
- Introduce a universal graph/plugin/build system: exceeds the supported use cases and duplicates ecosystem tools.

## Consequences

Pre-v1 breaking references are allowed by the owner requirement, but historical releases and committed decisions still need explicit compatibility. Cargo/npm/Python outputs can share an owner without pretending their native versions or registry evidence are interchangeable. Go/NuGet remain design examples, not shipped adapters.

## Sources

- Owner scope and canonical-identity discussion, 2026-09-26; [REQ-DX-V1](../specs/REQ-DX-V1.md).
- [Scoped model and primary-source prior art](../architecture/polyglot-v1.md).
- [System findings and verification limits](../projects/V1-FINDINGS.md).
