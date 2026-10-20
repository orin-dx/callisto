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

- `aggregate::resolve_target_package` resolves changeset entries through `IdentityIndex::resolve_id`, which maps qualified native distribution names to their owner and rejects ambiguous bare names. `status` attributes changeset entries and checks their well-formedness through the same resolver. `PackageId::resolve_unique` scans display ids, is deprecated, and is not equivalent for dual-manifest packages with different native names.
- `[[package]]` rules use `config::resolve::resolve_package_config`: two linear passes over the rules in declaration order, prefixed rules first, then bare. It never sorts the rules. A bare rule that reaches a promoted name is an `AmbiguousName` error, using `ResolvedConfig::promoted_siblings`.
- `[[package-set]]` is a fallback applied in `ManifestWalkResolver::build` (`crates/callisto-graph/src/walk.rs`) only when no `[[package]]` rule matched. Patterns (`config/pattern.rs`) are globs over the name with an optional ecosystem prefix, tested against every ecosystem the package has a manifest in.

## Loading a workspace

`Workspace::load` (`crates/callisto-graph/src/lib.rs`):

1. `config::load` reads `callisto.toml` (absent means empty) and calls `config::resolve`, which does all validation and defaulting. Callers holding a `RawConfig` in memory call `resolve` directly.
2. `ManifestWalkResolver::build` discovers projects, assigns ids (`IdentityIndex`), attaches platform packages and applies per-package config.
3. `GroupTable::resolve` binds group members to ids, then names promoted into several ecosystems are recorded as `promoted_siblings`.
4. The tag index is built lazily, on the first `Workspace::tags` call.

A Cargo package inherits from the Cargo workspace that governs it, found the way Cargo finds it (`cargo::enclosing_workspace_manifest`): `package.workspace`, else the nearest manifest at or above the package with a `workspace` table, never above the workspace root. That workspace can be below the root, so the walk, the version guard, release-commit verification, lockfile refresh and `registry_argv` all resolve it per package. Cargo does not judge membership for inheritance, and neither does this lookup: a root that excludes or omits the package is a repository Cargo itself rejects.

`Workspace::load` returns only a complete workspace, and it is the only way to get one. Every manifest the walk cannot open (for its platform role, publish targets or dependencies) is recorded as an error-level `ManifestUnresolved` diagnostic, and `load` turns any of them into E313 naming each manifest once, so no command sees a roster smaller than the repository.

## Historical ownership

For a promoted package, `TagIndex::build` also considers bare-name tags. `history::sole_owner_at` picks the same-named candidate whose first canonical manifest path held a manifest declaring the name at the tag's commit. When none does, because the directory moved or the package gained a higher-priority manifest, it asks which ecosystems had a manifest declaring the shared name at that commit, found with `git grep` and confirmed by reading only the matching manifests. The owner is the one candidate whose own manifest in such an ecosystem has that native name today, taken from the identity index. Two or more matches, or none, mean no owner. `assign_promoted_pre_keys` applies the same check at the commit that added `pre.json`, and E245 reports a baseline with no unique owner. A bare key that matches one current package is checked too: if `pre.json` was added when a manifest of another ecosystem held the name and the package had none, the key pins nothing.

A demoted package, whose same-named sibling was removed, also considers the qualified tags of its first manifest's ecosystem and keeps the newest version across both families.

A rename that changes the native name, or a move that changes it too, leaves no evidence in Git. Declare it with `previous-tag-templates`; do not infer it from the directory key.

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
