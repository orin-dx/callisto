# 12. Canonical distribution references and explicit release ownership

Status: Proposed

This proposal revises ADR 11's selector promotion when implemented. ADR 11 remains the current decision, including its default tag; it is not superseded by this documentation change.

## Context

The owner selected Rust/Cargo, npm/pnpm and Python/PyPI as the current scope, with architectural room for Go and .NET/NuGet. The system audit found that display promotion leaks into dependency rewrites, historical ownership and shared tag/forge ordering. Fresh Cargo/Maturin projects also fail because one package requires one grammar. Evidence and implementation work are recorded in [the findings register](../projects/V1-FINDINGS.md).

## Proposed decision

- Require canonical slash-qualified distribution references (`cargo/name`, `npm/@scope/name`, `pypi/name`) in `callisto.toml` (groups, package rules, product and artifact packages), in report and matrix keys, and in command-line arguments. Reject the colon form everywhere. A `pypi/` reference uses the declared name. Native normalization belongs to the ecosystem.
- Changesets and `pre.json` keep native package names and resolve them when read. A changeset entry names the owner package, not a distribution, and the files stay readable by `@changesets/cli` (ADR 1).
- Separate release ownership, distribution identity, authoritative version source and external effects. Reuse existing types and graphs; do not add a general plugin framework or opaque IDs without need.
- Release ownership is derived, not stored: the owner of a package's shared tag, forge release and announcement is its first release id by ecosystem priority Cargo, npm, PyPI. The intent digest seals it and `release execute` re-derives it, so no owner field or wire change is needed for the current scope.
- Keep ADR 11's default tag, `{display id}@{version}`, and never rename existing tags. A package takes only the tags its commit shows it made, and `previous-tag-templates` declares a rename.
- Keep native names and manifest context for dependency edits, inherited/derived versions and publisher arguments.
- Within Callisto's release-operation graph, shared external effects have one owner and wait for every required publication/output of that owner: selecting one distribution of a package releases all of them, a product's announcement waits for the tag of each package whose asset it carries, and a dependent waits only for the dependency's distribution in its own ecosystem. For current ecosystems, public announcement follows registry completion; a future Go source tag can itself be publication.
- Native package-manager lifecycle hooks retain package-owner-defined behavior and environment. Callisto does not inspect, rewrite, suppress or reject hooks; effects initiated by owner-defined hooks are outside Callisto's release-operation graph.
- Reruns observe every required effect, including unfinished work after tag creation and missing Python files. Keep ADR 2's committed roster and ADR 3's no-journal recovery.
- Co-located Cargo and Maturin, whose Python version is derived from Cargo, is after v1.

Channel and private-registry policies are not decided here and are not release gates.

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
