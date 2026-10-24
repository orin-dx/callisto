# Scoped polyglot release model

Status: partly implemented. Canonical references, derived release ownership, per-distribution publish ordering, registry-before-tag ordering, PyPI file-set completion, platform packages before their owner, the `publish-url` split and prerelease npm dist-tags are current behavior ([ADR 12](../adr/0012-canonical-distribution-release-ownership.md)). Co-located Cargo/Maturin version derivation, build-input relationships, authenticated private PyPI, npm dist-tag promotion and the Go and NuGet adapters are design only. Owner scope: Rust/Cargo, npm/pnpm and Python/PyPI now; leave room for Go and .NET/NuGet without implementing their publishers. Current contracts remain in [the specs](../specs/INDEX.md). Remaining work is in [the roadmap](../projects/ROAD-TO-V1.md).

## Four concepts

| Concept | Responsibility | Not equivalent to |
| --- | --- | --- |
| Release owner | Source/version authority and the outputs released together | A display name or registry destination |
| Distribution | Canonical ecosystem-qualified identity and native package name | A source directory or import/module name |
| Version authority | Literal or supported inherited/derived source, plus each distribution's native representation | A grammar guessed from a JSON string |
| Release effect | One required external effect, its destination, evidence and prerequisites | A package tag proving all publications finished |

Keep existing types where they already express these responsibilities. A `PackageKey` locates a directory; it does not prove identity survives a directory move. Do not add opaque IDs or a public plugin protocol without a demonstrated need. The release owner is derived from existing types and sealed by the intent digest; no owner field or wire change is needed for the current scope.

## Canonical references and migration

- Configuration, generated config, machine reports and command-line arguments use only canonical `cargo/name`, `npm/@scope/name` or `pypi/name` references; bare and colon-form aliases are rejected. Changesets and `pre.json` keep native package names, resolved when read, because an entry names an owner package and the files stay readable by `@changesets/cli`. Native normalization belongs to the ecosystem; Python distribution names are not import names, and a `pypi/` reference uses the declared name.
- Short labels may remain in human output, but never become an alternate identity authority. Ecosystem-specific native names stay available for manifest rewrites and publisher arguments.
- A bare name in a changeset or `pre.json` resolves through established ownership; ambiguous input fails with an actionable diagnostic. Persisted release documents are already canonical-only.
- Adding or removing a same-named sibling, adding a canonical manifest or changing the primary manifest must not transfer another owner's history or regress the last release. A genuine rename/move needs a declared migration boundary, not guessing from current path existence.
- Keep [ADR 11](../adr/0011-directory-keyed-package-identity.md) in force, including its default tag `{display id}@{version}`, which [ADR 12](../adr/0012-canonical-distribution-release-ownership.md) keeps. Do not rename existing tags to canonicalize selectors; a native rename is declared with `previous-tag-templates`.

## Native versions and dependencies

A release owner can have several distribution outputs. Shared versioning is deliberate; co-location or a build dependency alone does not require version coupling. Co-located Cargo/Maturin is after v1; today it fails with E007, and separate-directory Cargo and Python packages are supported. The design for it: Cargo is the version authority and the Python distribution uses `dynamic = ["version"]`. Maturin does not map versions; it parses the Cargo version as PEP 440, so Callisto computes the expected Python version with the `pep440_rs` crate and verifies the built metadata. Several SemVer prereleases are not faithful in PEP 440 (`pre`, `c` and `rc` collapse to `rc`; `dev` and `post` change the sort order; `+build` is a local version PyPI rejects), so accept only stable and `-alpha.N`, `-beta.N`, `-rc.N` and refuse the rest, and snapshots, before any write. Refuse a static Python version beside Cargo, which maturin does not check. Do not introduce a universal reversible version converter or arbitrary SCM-derived backends.

Reuse the native dependency graph and derived release-operation DAG. Preserve each edge's manifest, declared key/alias, resolved distribution, constraint and dependency kind. A native Cargo edge must not become a dependency on every distribution produced by the same source package.

- Native registry dependency: update that ecosystem's reference and derive only relevant publication prerequisites. Range coverage, version cascade and publication order are distinct decisions.
- Embedded/build input: the consuming artifact needs those bytes or sources. It does not necessarily need that input published to Cargo, npm or PyPI. Builders and CI retain build ordering; add explicit cross-project input relationships only for a supported concrete case.
- Version coupling: retain fixed/linked policies separately from dependency edges; do not infer a major bump of every wrapper from an internal-language change.
- Runtime services and arbitrary cross-language discovery are outside the release graph's scope.

For pnpm, retain the difference between `workspace:*`, explicit workspace ranges, aliases, relative paths and catalogs. Let pnpm perform pack-time transformations. For Python, preserve extras, markers and native specifiers; document direct-URL and dynamic-backend support rather than treating them as ordinary ranges. Cargo inherited versions/dependencies require their authoritative root paths in both live and historical reads.

## Effects and completion

For current registry ecosystems, the default is required registry publications confirmed before a shared tag and public GitHub announcement. A draft and uploads may be preparatory effects; public publication waits for all required outputs of that release owner. This is Callisto's policy, not a universal ecosystem order.

Within Callisto's release-operation graph, one physical tag, forge release or upload has one owner even when several distributions share it. Native package-manager lifecycle hooks retain package-owner-defined behavior and environment. Callisto does not inspect, rewrite, suppress or reject those hooks; any effects they initiate are outside Callisto's graph and remain the package owner's responsibility. For example, NAPI `prepublishOnly` may publish platform packages or GitHub assets while Callisto also performs configured release effects.

A retry retains the authorized roster and observes every remaining effect, even when its tag or some registry files already exist. Keep execution state in memory; the immutable intent, original artifact manifest and providers supply recovery evidence. No journal, automatic version bump, destructive rollback or changed replacement bytes under an already-published version.

PyPI completion is the declared file set for the release, not the first matching wheel or sdist. Required platform packages must exist before their npm owner publishes. Missing, conflicting, yanked and unobservable results must not be collapsed into complete success. Use hashes where needed to distinguish intended files from conflicting bytes; do not make an unapproved universal registry-checksum requirement a v1 gate.

Registry identity is separate from read/upload routes and native authentication configuration. Specify public PyPI first; decide authenticated private indexes and HTML-only indexes explicitly. Mutable npm dist-tags are channel labels, not distribution versions; channel verification/promotion policy is an owner decision, not implicit rollback.

## Future compatibility examples

- Go: a source tag is a publication effect; a v2+ module generally has a new major-qualified module path, and subdirectory modules need their native tag prefix. Do not require every ecosystem to publish before its source tag or carry its version in a manifest.
- NuGet: package-version normalization differs from SemVer; package version is not assembly/file version. A single package can contain runtime-identifier and target-framework assets. Do not assume every native target is a Rust triple or every platform is a separate distribution.
- Cross-language wrappers: native libraries, subprocess tools and generated bindings are concrete inputs to a native distribution, not foreign package-manager dependency entries. Building Rust into a Python wheel does not by itself require a crates.io publication.

These are model checks only. Go/NuGet discovery, rewriting, packing, authentication and publication remain deferred.

## Prior art and research limits

| Tool | Established approach | Boundary relevant to Callisto |
| --- | --- | --- |
| Changesets | npm/pnpm graph-based publication, then tags for successful publications | Native JS release semantics, not a cross-registry transaction. [Publisher](https://github.com/changesets/changesets/blob/main/packages/cli/src/commands/publish/index.ts) |
| Knope | Several versioned files can form one release package | Cargo+Python ownership stays simple; Python dependency rewriting is not supported by its documented format handler. [Ownership](https://knope.tech/tutorials/releasing-multiple-packages/), [formats](https://knope.tech/reference/config-file/packages/) |
| Nx Release | Project identities, release groups and ecosystem VersionActions/publishers | Reuse the separation, not the general build framework. Its Rust recipe still documents a legacy-versioning restriction; current plugin compatibility needs a runtime check. [Configuration](https://nx.dev/docs/reference/nx-json), [Rust recipe](https://nx.dev/docs/kb/publish-rust-crates) |
| release-please | Polyglot strategies, native workspace plugins and separate registry publishing | Its example publishes npm after tags/releases already exist; that is not Callisto's required completion policy. [Manifest model](https://github.com/googleapis/release-please/blob/main/docs/manifest-releaser.md) |

Sources checked on 2026-09-26: [Maturin version authority](https://www.maturin.rs/metadata), [pnpm workspace transformations](https://pnpm.io/workspaces), [catalogs](https://pnpm.io/catalogs), [Python name normalization](https://packaging.python.org/en/latest/specifications/name-normalization/), [PEP 440](https://packaging.python.org/en/latest/specifications/version-specifiers/), [PyPI one-file uploads](https://docs.pypi.org/api/upload/), [per-file index evidence](https://packaging.python.org/en/latest/specifications/simple-repository-api/), [NAPI publication hooks](https://napi.rs/docs/cli/pre-publish), [Go module versions](https://go.dev/doc/modules/version-numbers), [NuGet normalization](https://learn.microsoft.com/en-us/nuget/concepts/package-versioning), [NuGet native assets](https://learn.microsoft.com/en-us/nuget/create-packages/native-files-in-net-packages).

Competitor release workflows were inspected, not executed. Fresh co-located static and dynamic Cargo/Maturin discovery fixtures both reproduced E007, and the maturin 1.15.0 metadata hook agreed with `pep440_rs` on every version tried. Partial-upload fault injection shipped with the PyPI file-set work. Native pack/install tests remain open in [the roadmap](../projects/ROAD-TO-V1.md).

## Non-goals

No universal package manager, build scheduler, five public graph types, arbitrary interoperability inference, external service graph, cross-registry atomic transaction, public adapter framework or persisted execution-state mechanism. Expand the model only when a supported fixture demonstrates a missing responsibility.
