# Config Resolution — Semantic Model

Sources: `crates/callisto-graph/src/config/resolve.rs`, `crates/callisto-graph/src/config/pattern.rs`,
         `crates/callisto-graph/src/walk.rs`

## ResolvedConfig

The parsed, validated representation of `callisto.toml`. Key fields for package override:

```rust
pub struct ResolvedConfig {
    // Per-package override rules from [[package]] blocks, in TOML declaration order.
    pub packages: Vec<(PackageId, PackageConfig)>,

    // Bulk override rules from [[package-set]] blocks, in TOML declaration order.
    // Fallback when no [[package]] rule matches. Uses glob patterns over package names.
    pub package_sets: Vec<(PackagePattern, PackageConfig)>,

    pub registries: BTreeMap<RegistryKey, RegistryConfig>,
    pub groups: GroupTable,
    // ... other fields
}
```

## PackageConfig

The override payload applied to a matched package:

```rust
pub struct PackageConfig {
    pub release_trigger: Option<ReleaseTrigger>,
    pub tag_template: Option<TagTemplate>,
    pub changelog: Option<PathBuf>,
    pub publish_to: Option<Vec<PublishTarget>>,
}
```

All fields are `Option` — only explicitly set fields override defaults. Unset fields fall back
to the package's manifest-inferred values.

## PackagePattern

Source: `crates/callisto-graph/src/config/pattern.rs`

Wraps `globset::GlobMatcher` for use in `[[package-set]]` blocks.

```rust
pub struct PackagePattern { raw: String, matcher: GlobMatcher }
impl PackagePattern {
    pub fn parse(s: &str) -> Result<Self, globset::Error>
    pub fn matches(&self, id: &PackageId) -> bool  // checks id.name() only, not ecosystem
    pub fn as_str(&self) -> &str
}
```

`matches()` tests `id.name()` against the glob pattern. It is intentionally ecosystem-agnostic:
`[[package-set]] match = "foo-*"` applies to `cargo:foo-bar`, `npm:foo-bar`, etc. all at once.

## Rule Application in walk.rs

For each discovered package, `ManifestWalkResolver::build` resolves an override in two steps:

### Step 1: [[package]] rule lookup

Current code (first-match-wins, declaration order):
```rust
let pkg_override = cfg.packages.iter()
    .find(|(pattern, _)| pattern.matches(&id))
    .map(|(_, cfg)| cfg);
```

Track E fix (specificity-ordered — prefixed beats bare, regardless of declaration order):
```
Collect all matching [[package]] rules.
If any are Prefixed-pattern matches → use the first Prefixed match.
If none are Prefixed → use the first Bare match.
If no matches → None.
```

### Step 2: [[package-set]] fallback

Only consulted when Step 1 produced None:
```rust
let set_override = if pkg_override.is_none() {
    cfg.package_sets.iter()
        .find(|(pattern, _)| pattern.matches(&id))
        .map(|(_, cfg)| cfg)
} else {
    None
};
let active_override = pkg_override.or(set_override);
```

### Priority order (highest to lowest)

1. `[[package]]` with Prefixed PackageId pattern (e.g. `match = "cargo:foo"`)
2. `[[package]]` with Bare PackageId pattern (e.g. `match = "foo"`)
3. `[[package-set]]` with matching glob (e.g. `match = "foo-*"`)
4. Manifest-inferred defaults (publish_to from the manifest itself, Changeset trigger, etc.)

## Cross-Ecosystem Diagnostic (Track E Fix 2)

After the packages loop in `ManifestWalkResolver::build`:

For each Bare PackageId in `cfg.packages`, if the set of ecosystems of packages it matched
has size > 1, push one diagnostic to `diagnostics`:

```
"[[package]] rule `{pattern}` matches packages in multiple ecosystems ({list});
use an ecosystem-prefixed pattern like `cargo:{name}` if you intend only one ecosystem"
```

`[[package-set]]` rules are never checked for this diagnostic.

## PublishTarget Semantics

`PublishTarget::None` explicitly suppresses publishing.
An empty `publish_to` defaults to the manifest-inferred targets.
`publish_to = ["none"]` in callisto.toml → `vec![PublishTarget::None]` → excluded from publish plan.
