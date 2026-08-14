# Error Taxonomy — Semantic Model

Source: `crates/callisto-graph/src/error.rs`

## GraphError Variants

### With E-codes (user-facing, miette Diagnostic)

| Code | Variant | When to use |
|---|---|---|
| E100 | `DuplicatePackage { id, paths }` | Same PackageId discovered at two different workspace paths |
| E101 | `SplitIdentity { path, ids }` | A single manifest path declares two conflicting PackageIds |
| E102 | `UnknownPackage { id }` | A PackageId referenced in config or a plan is not in the workspace |
| E103 | `AmbiguousName { name, candidates }` | A bare name matches packages in 2+ ecosystems; caller must use prefixed form |
| E104 | `Cycle { cycle }` | Dependency cycle detected in the workspace graph |
| E105 | `CascadeNotConverged { iterations }` | Cascade failed to reach a fixed point |
| E106 | `FixedGroupDivergent { group, members }` | Fixed group members have different on-disk versions |
| E107 | `GroupGrammarMismatch { group, members }` | Fixed group members use incompatible versioning grammars |
| E108 | `MissingGroupMember { group, member }` | A group lists a member that is not in the workspace |
| E109 | `ConflictingGroupMembership { package, groups }` | A package is listed in two or more groups |
| E114 | `PreJson(PreJsonError)` | Failed to parse .changeset/pre.json |
| E115 | `PreJsonRead { message }` | Failed to read .changeset/pre.json |
| E116 (ConfigError) | `InvalidChangesetsDir { dir }` | changesets.dir contains `..` path components |
| E117 | `UnexpectedManifestVersion { path, expected_from, expected_to, found }` | Manifest version is neither bump.from nor bump.to; requires human intervention |

### Without E-codes (transparent or internal)

| Variant | Source / Notes |
|---|---|
| `Locate(LocateError)` | Transparent from locate module |
| `Manifest(ManifestError)` | Transparent from callisto-manifests |
| `Config(ConfigError)` | Transparent; see ConfigError section below |
| `Format(ParseError)` | Transparent from callisto-format |
| `ParseChangeset { path, source }` | Changeset file parse failure (not transparent — adds path context) |
| `Bump(BumpError)` | Transparent from callisto-format |
| `Changelog(ChangelogError)` | Transparent from callisto-changelog |
| `Conventional(ConventionalError)` | Feature-gated; transparent |
| `TagTemplate(TagTemplateError)` | Not transparent |
| `VersionParse(VersionParseError)` | Not transparent |
| `Model(ModelError)` | Transparent from callisto-model |
| `Vcs(VcsError)` | Transparent from callisto-vcs |
| `Command(CommandError)` | Not transparent; wraps git/fs failures |
| `OnDiskVersionDrift { package, expected, found }` | On-disk version changed since plan was generated |
| `WorkspaceVersionConflict { root_manifest, details }` | Workspace root has conflicting version updates |
| `GrammarMismatch { from, to, source }` | Incompatible versioning grammars on a dependency edge |

## ConfigError Variants

ConfigError is always wrapped in `GraphError::Config` (transparent). All ConfigError variants
surface to the user as GraphError.

### With E-codes

| Code | Variant | When to use |
|---|---|---|
| E110 | `Read { path, message }` | Failed to read callisto.toml or moon.yml |
| E111 | `ParseToml { path, message }` | Invalid TOML syntax in callisto.toml |
| E113 | `InvalidChangelogPath { pattern, value }` | `changelog` on `[[package]]`/`[[package-set]]` escapes the package root (previously mis-numbered E117, colliding with `GraphError::UnexpectedManifestVersion`; renumbered) |
| E116 | `InvalidChangesetsDir { dir }` | changesets.dir escapes workspace root |

### Without E-codes

| Variant | Status | When to use |
|---|---|---|
| `PackageMatchedNothing { pattern }` | Defined but NEVER EMITTED — known gap | A [[package]] rule matched no packages after workspace walk |
| `PackageSetMatchedNothing { pattern }` | Defined but NEVER EMITTED — known gap | A [[package-set]] rule matched no packages |
| `OverlappingPackageSets { package, patterns }` | Defined but NEVER EMITTED — known gap | A package is claimed by two [[package-set]] rules |
| `ConflictingGroupNames { group, other, member }` | Active | Same member in two groups |
| `EmptyGroup { group }` | Active | A group has no members |
| `DuplicateGroupName { group }` | Active | Two groups with the same name |
| `UnknownRegistry { key }` | Active | publish-to references a registry key not in [registries.*] |
| `UnknownKey { path, key }` | Active | serde deny_unknown_fields rejection |
| `InvalidBumpSeverity { found }` | Active | cascade.bump-severity is not "patch" or "minor" |
| `InvalidPreMajorInference { found }` | Active | pre-major-inference is not a valid value |
| `Tag(TagTemplateError)` | Active | Invalid tag template syntax |
| `VersionParse(VersionParseError)` | Active | Invalid version string in config |

## Conventions

- Use `GraphError::Command(CommandError::Io { ... })` for filesystem I/O errors that aren't
  manifest parse failures.
- `#[diagnostic(transparent)]` variants inherit the inner type's miette diagnostic code.
  Do NOT add `code()` to a transparent variant.
- `#[allow(clippy::result_large_err)]` is set at the enum level — do not add it per-variant.
- New variants go at the bottom of the enum unless they belong to an existing logical group.
- `#[non_exhaustive]` is on both enums — external crates cannot exhaustively match them.
