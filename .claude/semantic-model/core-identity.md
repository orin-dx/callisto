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

Parsed by `PackageId::parse(s)`. Accepted separators: `:` or `/` after a known ecosystem prefix.
`@myorg/foo` is Bare (slash is part of npm scope, not an ecosystem prefix).
`npm/@myorg/foo` and `npm:@myorg/foo` are both Prefixed { Npm, "@myorg/foo" }.

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

Bare as ecosystem wildcard is intentional: napi-rs packages share a name across Cargo and Npm
and are always versioned together. `Bare("foo")` correctly matches both `cargo:foo` and `npm:foo`.

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

`Ecosystem::from_prefix(s)` recognizes: "cargo", "npm", "pypi", "python", and others.
`ecosystem.prefix() -> &str` returns the canonical prefix string.

## GroupName / RegistryKey

Both are `String` newtypes with no validation beyond non-emptiness.

Well-known RegistryKey constants:
- `RegistryKey::CRATES_IO` = "cratesIo"
- `RegistryKey::NPM` = "npm"
- `RegistryKey::PYPI` = "pypi"
- `RegistryKey::NUGET` = "nuget"

## Track E Design Decision (Option 3)

**Do NOT change `PackageId::matches()`.**

The fix is at the call sites, not in the type:

Fix 1 — Specificity ordering in `[[package]]` rule application (`walk.rs`):
When multiple `[[package]]` rules match the same package, a `Prefixed` pattern rule beats a
`Bare` pattern rule, regardless of declaration order in `callisto.toml`. Among same-specificity
matches, first-match-wins (declaration order).

Fix 2 — Cross-ecosystem diagnostic (`walk.rs`, after packages loop):
A `Bare` PackageId in `cfg.packages` that matches packages in >1 ecosystem emits one diagnostic.
`[[package-set]]` rules are exempt — multi-ecosystem is their explicit purpose.

See `.claude/specs/track-e-specificity.json` for the full testable acceptance criteria.
