# Package identity

How the workspace names packages, why bare names match across ecosystems, and where platform packages are wired in. Behavior: [`docs/specs/workspace.json`](../specs/workspace.json). Config keys: [`docs/config.md`](../config.md). Platform ownership decision: [ADR 7](../adr/0007-platform-packages-owned-through-optional-dependencies.md).

## Terms

- Dual-manifest package: one directory with both `Cargo.toml` and `package.json` (e.g. a napi addon); one package, two release identities.
- Co-located platform manifest: an `os`+`cpu` `package.json` in the same directory as its owner's `Cargo.toml`.
- Attached platform package: an `os`+`cpu` `package.json` in its own directory, owned via another package's `optionalDependencies`.

## `PackageId` (`crates/callisto-model/src/identity.rs`)

| Form | Example | `ecosystem()` |
| --- | --- | --- |
| `Bare(name)` | `foo`, `@scope/foo` | `None` |
| `Prefixed { ecosystem, name }` | `cargo:foo`, `npm/@scope/foo` | `Some(e)` |

- `parse` treats text before the first `:` or `/` as a prefix only when `Ecosystem::from_prefix` knows it, so `@scope/foo` stays bare.
- `display_name` renders a prefixed id as `ecosystem/name`.
- `matches` is a weak "could be the same package" relation, not equality:
  - Different names never match.
  - A bare id matches any id with the same name, in either direction: bare is an ecosystem wildcard.
  - Two prefixed ids match only when their ecosystems are equal.
- Why bare is a wildcard: a dual-manifest package has one name in several ecosystems and one version, so `foo` must reach `cargo:foo` and `npm:foo` alike.

## Resolving ambiguity

Do not change `matches`. A polyglot workspace can hold `cargo:foo` and `npm:foo` as separate packages; ambiguity is resolved where a lookup happens.

- Lookups use `PackageId::resolve_unique`: collect every match, error on two or more. Callers: `aggregate::resolve_target_package` (changeset entries) and `commands/status.rs`.
- `[[package]]` rules use `config::resolve::resolve_package_config`: two linear passes over the rules in declaration order, prefixed rules first, then bare. It never sorts the rules. A bare rule that reaches a promoted name is an `AmbiguousName` error, using `ResolvedConfig::promoted_siblings`.
- `[[package-set]]` is a fallback applied in `ManifestWalkResolver::build` (`crates/callisto-graph/src/walk.rs`) only when no `[[package]]` rule matched. Patterns (`config/pattern.rs`) are globs over the name with an optional ecosystem prefix, tested against every ecosystem the package has a manifest in.

## Loading a workspace

`Workspace::load` (`crates/callisto-graph/src/lib.rs`):

1. `config::load` reads `callisto.toml` (absent means empty) and calls `config::resolve`, which does all validation and defaulting. Callers holding a `RawConfig` in memory call `resolve` directly.
2. `ManifestWalkResolver::build` discovers projects, assigns ids (`IdentityIndex`), attaches platform packages and applies per-package config.
3. `GroupTable::resolve` binds group members to ids, then names promoted into several ecosystems are recorded as `promoted_siblings`.
4. The tag index is built lazily, on the first `Workspace::tags` call.

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
