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

## Load and resolve

`config::load(root)` reads and parses `callisto.toml` (absent means empty) into `RawConfig`, then calls `config::resolve(root, RawConfig)`, which does all validation and defaulting. Callers holding an in-memory `RawConfig` call `resolve` directly.

`[release]`: `product-package`, `forge-repository` (`owner/repo`, optional at load, required to plan: E198), `[[release.artifact]]`. Legacy `[release.profiles.production].forge-repository` migrates when the top-level key is absent; both set and different, or any other profile name, is E197. Legacy `registry-routes` is parsed and ignored. `[init]` still parses; `resolve` ignores it.

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
pub struct PackagePattern { raw: String, ecosystem: Option<Ecosystem>, matcher: GlobMatcher }
impl PackagePattern {
    pub fn parse(s: &str) -> Result<Self, globset::Error>  // optional `cargo:`/`npm:`/`pypi:` prefix
    pub fn ecosystem(&self) -> Option<Ecosystem>
    pub fn matches(&self, id: &PackageId) -> bool
    pub fn matches_in_ecosystems(&self, name: &str, ecosystems: &[Ecosystem]) -> bool
}
```

An unprefixed pattern (`match = "foo-*"`) matches the name in any ecosystem. A prefixed pattern (`match = "cargo:foo-*"`) matches only packages discovered in that ecosystem; `walk.rs` passes each package's discovered ecosystems to `matches_in_ecosystems`.

## Rule Application in walk.rs

For each discovered package, `ManifestWalkResolver::build` resolves an override in two steps.

### Step 1: [[package]] rule lookup

`config::resolve::resolve_package_config` (two-pass specificity):
- Pass 1: first rule, in declaration order, with an ecosystem prefix that matches.
- Pass 2: only if pass 1 found nothing, first rule that matches. A bare rule matching a name promoted into several ecosystems returns `GraphError::AmbiguousName`.

### Step 2: [[package-set]] fallback

Only consulted when Step 1 produced `None`: the first `[[package-set]]` pattern, in declaration order, that matches. Every matching pattern is also recorded for the zero-match diagnostic, even when a `[[package]]` rule shadows it.

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
