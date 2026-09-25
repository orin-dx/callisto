# Core Identity — Semantic Model

Source: `crates/callisto-model/src/identity.rs`

## PackageId

Two variants:

```rust
pub enum PackageId {
    Bare(String),                                    // "foo", "@myorg/pkg"
    Prefixed { ecosystem: Ecosystem, name: String }, // "cargo:foo", "npm:@myorg/pkg"
}
```

Parsed by `PackageId::parse(s)`. Accepted separators: `:` or `/` after a known ecosystem prefix. `@myorg/foo` is Bare (slash is part of npm scope, not an ecosystem prefix). `npm/@myorg/foo` and `npm:@myorg/foo` are both Prefixed { Npm, "@myorg/foo" }.

Key methods:
- `name() -> &str` — package name without ecosystem prefix
- `ecosystem() -> Option<Ecosystem>` — None for Bare, Some(e) for Prefixed
- `display_name() -> String` — canonical form: bare as-is, prefixed as `ecosystem/name`
- `matches(&other) -> bool` — see invariants below

### matches() Invariants

These invariants are intentional and must not be changed:

1. `Bare(x).matches(Bare(x))` → true (same name)
2. `Bare(x).matches(Prefixed(e, x))` → true (bare is ecosystem wildcard)
3. `Prefixed(e, x).matches(Bare(x))` → true (symmetric)
4. `Prefixed(e1, x).matches(Prefixed(e2, x))` → true IFF e1 == e2
5. `Bare(x).matches(Bare(y))` where x ≠ y → false
6. Any id with name x does NOT match any id with name y where x ≠ y

Bare as ecosystem wildcard is intentional: napi-rs packages share a name across Cargo and Npm and are always versioned together. `Bare("foo")` correctly matches both `cargo:foo` and `npm:foo`.

### Caller Contract for Polyglot Workspaces

When using `matches()` for lookup where ambiguity is possible:
- Collect ALL matches (not just the first)
- If 2+ matches have different ecosystems → return `GraphError::AmbiguousName`
- Example: `resolve_target_package` in `aggregate.rs`

When using `matches()` for config rule application:
- See config-resolution.md for the specificity ordering that resolves polyglot ambiguity

## Ecosystem

```rust
pub enum Ecosystem { Cargo, Npm, Pypi, ... }
```

`Ecosystem::from_prefix(s)` recognizes: "cargo", "npm", "pypi", "python", and others. `ecosystem.prefix() -> &str` returns the canonical prefix string.

## GroupName / RegistryKey

Both are `String` newtypes with no validation beyond non-emptiness.

Well-known RegistryKey constants:
- `RegistryKey::CRATES_IO` = "cratesIo"
- `RegistryKey::NPM` = "npm"
- `RegistryKey::PYPI` = "pypi"
- `RegistryKey::NUGET` = "nuget"

## Rule Specificity Decision

**Do NOT change `PackageId::matches()`.**

The fix is at the call sites, not in the type:

Fix 1 — Specificity ordering in `[[package]]` rule application (`walk.rs`): When multiple `[[package]]` rules match the same package, a `Prefixed` pattern rule beats a `Bare` pattern rule, regardless of declaration order in `callisto.toml`. Among same-specificity matches, first-match-wins (declaration order).

Fix 2 — Cross-ecosystem diagnostic (`walk.rs`, after packages loop): A `Bare` PackageId in `cfg.packages` that matches packages in >1 ecosystem emits one diagnostic. `[[package-set]]` rules are exempt — multi-ecosystem is their explicit purpose.

See `docs/specs/track-e-specificity.json` for the full testable acceptance criteria.

## npm platform packages -- Case E (`walk.rs::platform_owners`)

A `package.json` with `os`+`cpu` (`NpmRole::Platform`) whose `name` appears in exactly one other npm package's `optionalDependencies` is a `ManifestRole::Platform` manifest of that owner, never a `Package` (never tagged). The owner signal is `optionalDependencies`, so napi addons and esbuild-style native CLIs behave the same.

- Discovery also considers platform dirs outside the npm workspace globs (`ProjectLocator::projects_and_platform_candidates`); those attach or stay invisible.
- A workspace-member platform with zero or several owners stays its own package and gets `platform-package-without-owner`. Never guess an owner.
- A platform `package.json` sharing a directory with a `Cargo.toml` is Case D: it already belongs to that directory's package and is not re-attached.
- `IdentityIndex::platform` maps the platform's own name to its owner; `IdentityIndex::attached_platforms(owner)` excludes Case D platform manifests.
- Versioning: `commands::version::platform_version_writes` (version + snapshot) writes the owner's target version into each attached platform manifest (and any `[[fixed-group]]` platform member) and updates the owner's `optionalDependencies` pins. No fixed group needed.
- A Case D package's per-ecosystem release identity is each manifest's own native name (`release_package_ids`), not `PackageId::name()`.
- Release: `derive` emits one `ReleaseOperationRole::PlatformPublish { registry, platform }` per attached platform of each selected owner with an npm target. Its id's package/version are the owner's, so it passes `validate_operation_roster`; it is never in `selected`, so it gets no tag, forge, or artifact op. Every platform op is a prerequisite of the owner's `RegistryPublish`. It routes to the registry provider (`npm view <platform>@<version>`), publishing by directory (`npm_publish_directory_argv`: `npm publish <abs dir>` for every package manager). `ReleaseIntentV1::SCHEMA_VERSION` = 4 for this role.
- Attached platform manifests contribute no `publish_to` targets to their owner (walk.rs).
