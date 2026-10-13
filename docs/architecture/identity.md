# Package identity

How the workspace names packages, why bare names match across ecosystems, and where platform packages are wired in. Behavior: [`docs/specs/workspace.json`](../specs/workspace.json). Config keys: [`docs/config.md`](../config.md). Platform ownership decision: [ADR 7](../adr/0007-platform-packages-owned-through-optional-dependencies.md).

## Terms

- Dual-manifest package: one directory with several admitted canonical manifests; one graph package, one version grammar and several distribution identities. Cargo+npm is supported by the model; co-located Cargo+Python currently fails with E007 because their grammars differ, including a Maturin dynamic Python version.
- Co-located platform manifest: an `os`+`cpu` `package.json` in the same directory as its owner's `Cargo.toml`.
- Attached platform package: an `os`+`cpu` `package.json` in its own directory, owned via another package's `optionalDependencies`.

## `PackageId` (`crates/callisto-model/src/identity.rs`)

| Form | Example | `ecosystem()` |
| --- | --- | --- |
| `Bare(name)` | `foo`, `@scope/foo` | `None` |
| `Prefixed { ecosystem, name }` | `cargo:foo`, `npm/@scope/foo` | `Some(e)` |

- `parse` treats text before the first `:` or `/` as a prefix only when `Ecosystem::from_prefix` knows it, so `@scope/foo` stays bare.
- `display_name` renders a prefixed id as `ecosystem/name`.
- `PackageId` is the display and selector form; `Package::key()` is the workspace-relative directory of the first canonical manifest. It survives display promotion, not a directory move. `ReleasePackageId` names each canonical distribution as `ecosystem/name` using that manifest's own native name.
- Default tags use `display_name()`: `foo@{version}` while bare, `cargo/foo@{version}` after promotion. Promotion therefore also changes the namespace of newly created default tags (ADR 11).
- `matches` is a weak "could be the same package" relation, not equality:
  - Different names never match.
  - A bare id matches any id with the same name, in either direction: bare is an ecosystem wildcard.
  - Two prefixed ids match only when their ecosystems are equal.
- A dual-manifest package can have different native names. `IdentityIndex` resolves each ecosystem/name to its owner; the weak `matches` relation alone does not establish distribution identity or ownership.

## Resolving ambiguity

A workspace can hold `cargo:foo` and `npm:foo` as separate packages; ambiguity is resolved where a lookup happens. Qualified selectors work before and after promotion. Canonical-only new references are planned in [the v1 design](polyglot-v1.md), not implemented by today's parser.

- `aggregate::resolve_target_package` resolves changeset entries through `IdentityIndex::resolve_id`, which maps qualified native distribution names to their owner and rejects ambiguous bare names. Some status helpers still use `PackageId::resolve_unique`, a scan over display ids; these are not equivalent for dual-manifest packages with different native names.
- `[[package]]` rules use `config::resolve::resolve_package_config`: two linear passes over the rules in declaration order, prefixed rules first, then bare. It never sorts the rules. A bare rule that reaches a promoted name is an `AmbiguousName` error, using `ResolvedConfig::promoted_siblings`.
- `[[package-set]]` is a fallback applied in `ManifestWalkResolver::build` (`crates/callisto-graph/src/walk.rs`) only when no `[[package]]` rule matched. Patterns (`config/pattern.rs`) are globs over the name with an optional ecosystem prefix, tested against every ecosystem the package has a manifest in.

## Loading a workspace

`Workspace::load` (`crates/callisto-graph/src/lib.rs`):

1. `config::load` reads `callisto.toml` (absent means empty) and calls `config::resolve`, which does all validation and defaulting. Callers holding a `RawConfig` in memory call `resolve` directly.
2. `ManifestWalkResolver::build` discovers projects, assigns ids (`IdentityIndex`), attaches platform packages and applies per-package config.
3. `GroupTable::resolve` binds group members to ids, then names promoted into several ecosystems are recorded as `promoted_siblings`.
4. The tag index is built lazily, on the first `Workspace::tags` call.

## Historical ownership

For a promoted package, `TagIndex::build` also considers bare-name tags. `sole_owner_at` checks which same-named candidate's first current canonical manifest path exists at the tag's commit; it does not read every historical manifest name. `assign_promoted_pre_keys` applies that ownership check at the commit that added `pre.json`, and E245 reports a baseline with no unique owner.

This is not complete rename, manifest-addition or demotion support. Removing a sibling can stop lookup of a newer qualified tag, and current paths can misidentify historic bare ownership. Those failures remain tracked in [the findings register](../projects/V1-FINDINGS.md); do not infer migration safety from the directory key alone.

## Platform packages in code

| Step | Where |
| --- | --- |
| Find candidates outside the npm workspace globs | `ProjectLocator::projects_and_platform_candidates` |
| Attach a platform to its single owner, or warn | `platform_owners` in `walk.rs` |
| Look up a platform's owner by the platform's own npm name | `IdentityIndex::platform` |
| List an owner's attached platforms (co-located ones excluded) | `IdentityIndex::attached_platforms` |
| Write the owner's version into platform manifests and pin `optionalDependencies` | `platform_version_writes` in `commands/version.rs` (version and snapshot) |
| Publish each platform before its owner | `commands/release/derive.rs`: one `PlatformPublish` operation per attached platform |

- A `PlatformPublish` operation carries the owner's package id and version, so it passes the intent's check that every operation belongs to a decision entry, and each one is a prerequisite of the owner's npm `RegistryPublish`.
- It gets no tag, forge or artifact operation of its own; it publishes by directory.
- Release identities come from `release_package_ids` (`commands/release_decision.rs`): one per canonical manifest, using that manifest's own name, so a dual-manifest package with different Cargo and npm names releases under both.
