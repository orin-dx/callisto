# callisto — crate specification

**Status:** IMPLEMENTED & VERIFIED (Revision 1 Implementation Spec)
**Date:** 2026-07-24
**Repo:** `github.com/orin-dx/callisto`
**Companions:** `docs/00-design.md` (canonical design), `docs/02-library-vs-moon-decision.md`
(the §0.1 resolution), `docs/01-research-brief.md` (prior-art survey)

---

## 1. About this document

### 1.1 Purpose

`00-design.md` says *what callisto does and why*. This document says *what an implementer
types*. It is exactly one level more concrete: every type, trait, function signature, error
variant, algorithm, and fixture obligation for all ten crates in §15's layout, organised by
crate, with each element traced back to the design-doc section that motivates it.

It is deliberately **not** a second design document. Where this spec and `00-design.md`
disagree, `00-design.md` wins and this document is the bug. Where `00-design.md` is silent
and a choice had to be made to produce a compilable shape, that choice is flagged inline as
`[SPEC DECISION, not in 00-design.md: …]` and collected in §11 for batch review.

### 1.2 Relationship to the other two documents

- **`00-design.md`** is the source of truth for semantics: the changeset format (§6), the
  cascade table (§7.4), group behaviour (§7.5), mutation ordering (§7.6), the publish-plan
  shape (§9.2), the JSON contract (§12.5), the invariants (§13), config (§14), the crate
  layout (§15), licensing (§16), and milestones (§17). Every section of this spec cites it.
- **`02-library-vs-moon-decision.md`** resolved §0.1 (Option C, narrowly defined) and
  produced four crate-boundary changes that this spec treats as settled input, not as open
  questions: `MoonProjectGraphResolver` is deleted; `ProjectLocator` splits discovery from
  edge cross-checking; `DependencyResolver` keeps `-> impl Iterator` and static dispatch and
  is justified by `callisto-fixtures`' in-memory implementor; plan/report value types live
  in the permissive `callisto-model` tier. It is also the authority for two behavioural
  positions this spec repeatedly leans on — the 0.x remap staying out of `bump_version`
  (§F.6), and the `#2207` one-function identity-resolution rule (§M.9.2, §MO.4.3).

### 1.3 How each crate section is organised

Every crate section below follows the same five-part shape, in this order:

1. **Purpose** — one-liner, license tier, milestone.
2. **Dependencies** — which other `callisto-*` crates, which key external crates, and what
   the crate deliberately does *not* depend on (several of those absences are CI-enforced).
3. **Types and traits** — full definitions, no elisions.
4. **Algorithms and processes** — the parts where the algorithm *is* the spec.
5. **Open notes** — fixture obligations, milestone slicing, "deliberately not owned by this
   crate", and the crate's index of `[SPEC DECISION]` flags.

**Numbering convention.** Top-level sections are numbered `§N` in the design doc's own
style. Within each crate section, subsections keep a stable crate-letter prefix — `M` for
`callisto-model`, `F` for `callisto-format`, `CM` for `callisto-manifests`, `C` for
`callisto-conventional`, `CL` for `callisto-changelog`, `G` for `callisto-graph`, `CLI` for
`callisto-cli`, `MO` for `callisto-moon`, `CF` for `callisto-fixtures`, `V` for
`callisto-vcs` — so that a cross-reference like §M.12.3 or §G.7.7 is unambiguous and stable
regardless of where the crate's section lands in the document order.

Design-doc references are bare (§7.4, §13 invariant 15). This document's own references
carry the crate letter (§M.12.3, §CM.4.4).

### 1.4 Crate dependency graph

Nine crates. Edges are `cargo` dependencies; the graph is acyclic apart from the
dev-dependency edges noted below, which Cargo permits and which this layout uses
deliberately.

```
                            ┌───────────────────────────────┐
                            │        callisto-model         │  MIT/Apache-2.0
                            │  types · traits · JSON contract│  no callisto-* deps
                            └───────────────────────────────┘
                              ▲      ▲       ▲       ▲      ▲
              ┌───────────────┘      │       │       │      └───────────────┐
              │                      │       │       │                      │
    ┌─────────────────┐   ┌──────────────────┐  ┌──────────────────┐  ┌──────────────────┐
    │ callisto-format │   │callisto-manifests│  │    callisto-     │  │    callisto-     │
    │  MIT/Apache-2.0 │   │      AGPL        │  │  conventional    │  │    changelog     │
    │ changesets fmt  │   │ per-ecosystem I/O│  │      AGPL        │  │      AGPL        │  │ native gitoxide  │
    └─────────────────┘   └──────────────────┘  └──────────────────┘  └──────────────────┘  └──────────────────┘
              ▲                      ▲                  ▲                       ▲                     ▲
              │                      │                  │ (optional             │                     │
              │                      │                  │  `inference` feature) │                     │
              └──────────┬───────────┴──────────────────┴───────────────────────┴─────────────────────┘
                         │
              ┌────────────────────────┐
              │     callisto-graph     │  AGPL — config · discovery · graph · aggregation ·
              │                        │  cascade · groups · tags · plan/apply · commands
              └────────────────────────┘
                     ▲            ▲
        ┌────────────┘            └──────────────┐
        │                                        │
┌──────────────────┐                    ┌──────────────────┐
│   callisto-cli   │◀───────────────────│  callisto-moon   │
│      AGPL        │  `wrapper` feature │      AGPL        │
│ argv · render ·  │  (argv defs +      │ extism plugin ·  │
│  process I/O     │   pure renderers)  │ MoonProjectLocator│
└──────────────────┘                    └──────────────────┘

┌──────────────────────────────────────────────────────────────────────────────┐
│ callisto-fixtures — AGPL, dev-only, `publish = false`                        │
│   default features → callisto-model only (corpus data + typed tables)        │
│   `graph` feature  → + format, manifests, conventional, changelog, graph     │
│                       (in-memory DependencyResolver, Scenario runner)        │
│   Every other crate dev-depends on it. `callisto-format`, `callisto-cli` and │
│   `callisto-moon` enable only the feature set that avoids a cycle (§CF.2).   │
└──────────────────────────────────────────────────────────────────────────────┘
```

**Edges that are absences, and are CI-enforced rather than conventional:**

| Absent edge | Why | Enforcement |
|---|---|---|
| any core crate → moon / `callisto-moon` | §0.1 rule 1, §13 inv. 26 | `xtask dep-audit` over `cargo metadata`'s resolve graph (§G.1.7) |
| `callisto-cli` → `callisto-manifests` (**direct** edge only; the transitive chain through `callisto-graph` is real and required) | §13 inv. 27's structural half — a CLI that cannot *name* `Manifest` cannot grow a manifest-writing path outside `apply_version_plan` | same audit (§CLI.9 item 5) |
| `callisto-model` → any `callisto-*` | keeps the permissive tier at the bottom of the graph | same audit |
| any crate → `octocrab`/`reqwest`/`tokio` | §9.5 removed HTTP from callisto's scope entirely | same audit |

**Milestone slicing of the graph.** v0.1 ships `model`, `format`, `manifests`, `changelog`,
`graph`, `cli`, `fixtures`. v0.2 adds `conventional` (and turns on `graph`'s `inference`
feature). v0.3 adds no crate — it fills in `graph::groups`/`graph::napi` and
`graph::tags::pre_cursor`. v0.4 adds `callisto-moon`. (§17.)

### 1.5 Conventions used throughout

- Every path held by a value type is **workspace-root-relative and UTF-8** (§M.1.3). The
  only absolute paths in the system are the ones an I/O boundary resolves at the moment of
  use.
- Every public type in `callisto-model` is `Send + Sync + 'static` (§M.1.5); other crates
  inherit that where a trait bound requires it.
- Errors are `thiserror`-derived, `#[non_exhaustive]` where a variant is plausibly added
  later, and `PartialEq + Eq + Clone` wherever a fixture needs to assert on one — which is
  why no error type in this workspace holds a `std::io::Error` directly.
- "Warn-by-default" always means a `Diagnostic` with `severity: Warning` and an
  `escalated_by` flag, never a printed string (§M.11.2).
- Code blocks are the spec. Doc comments inside them are normative, not illustrative.

---

## 2. `callisto-model`

**Purpose.** The vocabulary every other crate shares: package identity, versions,
severities, manifests, dependency specs, discovery types, tag templates, the exec seam,
diagnostics, and the plan/report value types that *are* callisto's public contract.

**License:** MIT OR Apache-2.0 (§16). **Milestone:** v0.1 (§17).

### M.1 Crate-level rules

#### M.1.1 Purpose and license tier

`callisto-model` is the bottom of the dependency graph and the permissive half of §16's
two-tier licensing. §16 is explicit about why the plan/report types live here rather than in
AGPL-tier `callisto-graph`: "they're the public JSON contract per P7, so MIT/Apache tier" —
the Action, a future Nx plugin, or an independent consumer can depend on the schema types
without touching AGPL code.

`callisto-model` depends on **no other `callisto-*` crate**. That is a structural property,
CI-enforced by the same `xtask dep-audit` job that enforces the moon-free rule (§G.1.7), and it
is what makes the permissive tier coherent: `callisto-model` + `callisto-format` together are a
self-contained, MIT/Apache, coordination-free primitive a third party can depend on without
buying into callisto's graph, cascade, or manifest opinions.

```toml
[package]
name = "callisto-model"
version = "0.1.0"
edition = "2021"
license = "MIT OR Apache-2.0"
description = "Shared types and traits for callisto: package identity, versions, manifests, dependency specs, and the versioned JSON report contract."
repository = "https://github.com/orin-dx/callisto"

[dependencies]
semver = { version = "1", features = ["serde"] }   # workspace-pinned, see below
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "1"

[dev-dependencies]
callisto-fixtures = { path = "../callisto-fixtures" }
```

`semver` is a dependency of this crate's `SemVer` *grammar implementation*, not a leaked public
type: `Version` (§M.4.1) is callisto's own grammar-tagged struct, and `semver::Version` never
appears in a public signature. Pinning it once at `[workspace.dependencies]` matters anyway,
because §6.2's byte-exactness claim for `bump_version` (§F.6.2) is meaningless if two crates in
the workspace disagree about what SemVer parsing and rendering do.

#### M.1.2 No I/O

**This crate performs no filesystem, process, or network I/O.** It declares the traits
through which I/O happens (`CommandRunner`, §M.10) and the value types I/O produces, and it
implements neither. Two consequences worth stating rather than leaving implicit:

- `wasm32-wasip1` conformance (§0.1 rule 2) is trivially satisfied here, which is why this
  crate is the cheapest place to prove the `wasmtime` fixture harness works at all
  (§M.17 item 8).
- Any future PR adding a `std::fs` or `std::process` call to this crate is a boundary
  violation, not a convenience — the same class of change §13 invariant 26's audit exists to
  catch for moon.

#### M.1.3 Paths are workspace-root-relative and UTF-8

> `[SPEC DECISION, not in 00-design.md: every `PathBuf`/`Path` in a `callisto-model` type is
> workspace-root-relative and valid UTF-8, enforced by validating constructors that reject
> otherwise (`ModelError::AbsolutePath`, `ModelError::NonUtf8Path`).]` 00-design.md never
> states a path convention. Two independent requirements force one: §0.1 rule 2's WASI
> preopened-directory sandbox makes an absolute host path unaddressable under
> `callisto-moon` (the decision doc names "unpreopened directories and absolute host paths"
> as exactly what a build-only check misses), and §12.5's JSON contract serialises paths, so
> a non-UTF-8 path would be unserialisable. Making the rule structural — in constructors,
> not in a review checklist — is P5. Absolute paths still exist at I/O boundaries
> (`callisto-manifests` resolves relative paths against a root it is handed,
> `CommandRunner::run` takes a real `cwd`); they simply never live inside a model value.

```rust
/// The one path constructor every model type that holds a path funnels through. Rejects
/// absolute paths and non-UTF-8 paths, and normalizes `.` and `..` components **lexically** —
/// never by touching the filesystem, which this crate does not do (§M.1.2) — so two spellings
/// of the same relative path compare equal.
pub fn workspace_relative(path: impl AsRef<Path>) -> Result<PathBuf, ModelError>;
```

Resolving a workspace-relative path against a real root is the *caller's* job, at the I/O
boundary (§CM.2, §CL.6, §MO.6).

#### M.1.4 Module layout

```
callisto-model/
└── src/
    ├── lib.rs          # crate docs, re-exports, SCHEMA_VERSION
    ├── path.rs         # workspace_relative
    ├── identity.rs     # PackageId, GroupName, GroupKind, RegistryKey, CommitSha
    ├── ecosystem.rs    # Ecosystem, PublishTarget, ReleaseTrigger
    ├── version.rs      # Version, VersionGrammar, VersionReq, VersionParseError,
    │                   #   GrammarMismatch
    ├── severity.rs     # Severity, SeverityParseError
    ├── package.rs      # Package, ManifestDecl, ManifestRole, ManifestFormat
    ├── dependency.rs   # DepKind, DepSpec, Coverage, DependencyEntry, DepEdge,
    │                   #   WorkspaceKind
    ├── discovery.rs    # ProjectRoot, DeclaredEdge, DeclaredEdgeKind
    ├── tag.rs          # TagTemplate, TagName, LastTag, LastTagSelection,
    │                   #   select_last_tag, TagTemplateError
    ├── exec.rs         # CommandRunner, CommandOutput, CommandError
    ├── diagnostic.rs   # ConfigKey, Diagnostic, DiagnosticSeverity, DiagnosticCode,
    │                   #   StrictFlag
    ├── plan.rs         # PublishPlan and friends
    ├── report.rs       # Report trait, VersionReport, StatusReport, SnapshotReport,
    │                   #   ComposePrBodyReport, ValidateReport, TagReport, InitReport
    └── error.rs        # ModelError, ManifestError
```

#### M.1.5 Auto traits

Every public type in this crate is `Send + Sync + 'static`, asserted by a compile test
(§M.17 item 7) rather than assumed. §M.14's table records, per type, where that bound is
actually load-bearing — most of them because `Manifest`, `ProjectLocator`,
`DependencyResolver`, and `CommandRunner` are all declared `Send + Sync` in §15, and a
non-`Sync` field would make an implementor un-writable.

### M.2 Identity — `identity.rs`

```rust
/// §5.1, §5.4. `Bare` when the name is unambiguous workspace-wide; `Prefixed` when the same
/// string names packages in more than one ecosystem.
///
/// A Case D (dual-published) package is **one** `PackageId`, not two — §5.4 is explicit that
/// `"cargo/foo": patch` and `"npm/foo": minor` in one changeset resolve to a single `minor`
/// bump written to both manifests. Prefixing is a *naming* affordance for changeset authors
/// and disambiguation, never an identity split. §G.4.3's Case D collapse and
/// `GraphError::SplitIdentity` (§M.13.3) exist to keep that true structurally.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub enum PackageId {
    Bare(String),
    Prefixed { ecosystem: Ecosystem, name: String },
}

impl PackageId {
    /// Parses `foo`, `@myorg/foo`, `cargo/foo`, `npm/@myorg/foo`,
    /// `maven/org.example:foo-core`.
    ///
    /// The prefix is recognised only when the segment before the first `/` is a known
    /// `Ecosystem::from_prefix` token. A leading segment that is *not* one is **neither an
    /// error nor a prefix**: `@myorg/foo` parses as `Bare("@myorg/foo")` (an npm scope is part
    /// of the name) and so does `not-an-ecosystem/foo`, because npm scoped names and
    /// path-shaped bare names both legitimately contain `/`. `maven/org.example:foo-core`
    /// parses as `Prefixed` with the colon form left intact in `name` (§5.4).
    ///
    /// The only hard failures are a leading `/`, an empty name after a valid prefix
    /// (`cargo/`), and the empty string.
    pub fn parse(s: &str) -> Result<Self, PackageIdParseError>;

    /// The shortest unambiguous rendering (§5.4's "on write, emit the shortest unambiguous
    /// form"): `Bare(n)` renders as `n`, `Prefixed { ecosystem, name }` as
    /// `{ecosystem}/{name}`. This is the string that appears in `.changeset/*.md`
    /// frontmatter, in `BumpRecord.package`, and in the pre-cursor ref name (§C.6).
    ///
    /// **It is not the ecosystem-native package name.** A plan entry's `name` field
    /// (§M.12.2) is the string `cargo publish -p` / `pnpm --filter` consumes; for a prefixed
    /// identity the two differ, and conflating them hands the calling workflow an
    /// unusable argument.
    pub fn display_name(&self) -> String;

    /// `None` for `Bare`.
    pub fn ecosystem(&self) -> Option<Ecosystem>;

    /// The identity's own name component, with any ecosystem prefix stripped: `Bare(n)` → `n`,
    /// `Prefixed { name, .. }` → `name`. Used for diagnostics, sorting (`Ord`, below), and
    /// glob/`match` comparison against config patterns.
    ///
    /// **This is not the ecosystem-native publish name, and must not be used as one.** For a
    /// Case D package the two ecosystems can legitimately declare *different* native names —
    /// `Cargo.toml` says `foo`, `package.json` says `@myorg/foo`, one `PackageId::Bare("foo")`
    /// (§G.4.3 blesses this shape explicitly) — and a single `&str` cannot be both. Plan
    /// entries (`CratePublish::name`, `NpmPublish::name`, `NpmMainPublish::name`, §M.12.2) and
    /// dependency-table keys therefore source their name from
    /// `IdentityIndex::native_name(id, ecosystem)` (§G.4.2), which is keyed by ecosystem and
    /// so can return both.
    pub fn name(&self) -> &str;
}

impl std::fmt::Display for PackageId { /* == display_name */ }

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum PackageIdParseError {
    #[error("package identity is empty")]
    Empty,
    #[error("package identity `{raw}` has ecosystem prefix `{prefix}` but no name after it")]
    EmptyNameAfterPrefix { raw: String, prefix: String },
    #[error("`{raw}` starts with `/`")]
    LeadingSlash { raw: String },
}
```

`Ord` is `(ecosystem_or_none, name)`, with every `Bare` sorting before every `Prefixed`. It is
load-bearing in exactly two places, both determinism requirements rather than semantics: the
cascade's attribution tie-break (§G.7.5) and `toposort`'s tie-break (§G.3.2). It is never a
priority ranking.

```rust
/// A `[[fixed-group]]`/`[[linked-group]]` name (§14). A newtype rather than a bare `String`
/// so a group name can never be passed where a `PackageId` is expected — the two are both
/// user-authored strings that name workspace things, and §14's parse-time group validation
/// (§G.5.5) reads better when the types keep them apart.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GroupName(pub String);

impl GroupName { pub fn as_str(&self) -> &str; }
impl std::fmt::Display for GroupName { /* … */ }

/// Fixed vs. linked (§7.5): whether a group's members always share the exact version
/// (`Fixed`, hard error on divergence) or only when jointly touched in the same release
/// (`Linked`, independent lines otherwise). Lives here, not in `callisto-graph` or
/// `callisto-changelog`, specifically so both can share one definition — an earlier draft
/// defined this independently in each of those two crates, which is precisely the
/// two-code-paths-for-one-concept shape invariant 25 (the `#2207` lesson) warns against,
/// just for a group's *kind* instead of a package's tag identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GroupKind { Fixed, Linked }

/// A `[registries.*]` table key (§14) — `"cratesIo"`, `"npm"`, or a user-declared third
/// registry. Always serialises as a plain string, which is what keeps `publishTo` a stable
/// JSON string in every plan (§M.12.2).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RegistryKey(pub String);

impl RegistryKey {
    pub const CRATES_IO: &'static str = "cratesIo";
    pub const NPM: &'static str = "npm";
    pub fn as_str(&self) -> &str;
}

/// A full 40-hex git object id. Validated on construction — a truncated or non-hex string is
/// rejected rather than stored, because `ReleaseEntry.sha` (§M.12.2) is consumed positionally
/// by §9.3's `git tag "$tag" "$sha"` loop and a malformed value fails there, far from its
/// origin.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct CommitSha(String);

impl CommitSha {
    pub fn parse(s: &str) -> Result<Self, ModelError>;
    pub fn as_str(&self) -> &str;
    /// First 7 characters — `git`'s own default abbreviation length, used by
    /// `callisto-changelog`'s commit bullets (§CL.5).
    pub fn short(&self) -> &str;
}
```

### M.3 Ecosystem, publish target, release trigger — `ecosystem.rs`

```rust
/// §5.1. Committed variants first; demand-gated variants are declared so the traits in §7
/// and §15 stay open (P4), with **no behaviour built behind them** (§2.2's scope discipline,
/// restated by §CM.7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Ecosystem {
    Cargo,
    Npm,
    // Demand-gated (§2.2), declared for forward compatibility, not implemented:
    Pypi,
    Go,
    Maven,
    NuGet,
    Deno,
    Jsr,
}

impl Ecosystem {
    /// The token `PackageId::parse` recognises as a prefix, and the token
    /// `PackageId::display_name` emits: `"cargo"`, `"npm"`, `"pypi"`, `"go"`, `"maven"`,
    /// `"nuget"`, `"deno"`, `"jsr"`.
    pub fn prefix(&self) -> &'static str;
    pub fn from_prefix(s: &str) -> Option<Self>;

    /// §7.7's per-ecosystem versioning grammar — the grammar this ecosystem's versions are
    /// read and compared under. `Cargo`/`Npm`/`Deno`/`Jsr`/`NuGet` → `SemVer`; `Pypi` →
    /// `Pep440`; `Maven` → `Maven`; `Go` → `SemVer` (Go's module versions are SemVer with a
    /// `v` prefix, which is a *tag* concern, not a grammar one). Total, so a future
    /// implementation has one place to change rather than N.
    pub fn version_grammar(&self) -> VersionGrammar;

    /// Whether v0.1–v0.4's committed scope actually implements this ecosystem (§2.2) — `true`
    /// for `Cargo` and `Npm` only. Used by `callisto-manifests::open` (§CM.2) and by `init`'s
    /// discovery narration (§G.11) to refuse clearly rather than half-supporting.
    pub fn is_implemented(&self) -> bool;
}
```

`Ord` is derived and its order is declaration order. It is used only as a tie-break inside
`PackageId`'s ordering (§M.2), never as a priority ranking.

```rust
/// §5.1.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum PublishTarget {
    CratesIo,
    Npm { registry: Option<String> },
    // demand-gated:
    Pypi { index: Option<String> },
    NuGet { source: Option<String> },
    /// Orphaned by §9.5's scope cut: creating a GitHub Release is a registry-publish-shaped
    /// action, and callisto has no HTTP client. Retained only so a calling workflow's own
    /// `gh release create` step can be named as a *planning* target; callisto never creates
    /// one.
    GitHubRelease,
    /// Internal-only, never published. `privatePackages.version = true` in a migrated
    /// `.changeset/config.json` maps here (§18 Q4).
    None,
}

impl PublishTarget {
    /// The `[registries.*]` key this target resolves to, or `None` for `GitHubRelease`/
    /// `None`. `CratesIo` → `RegistryKey::CRATES_IO`; `Npm { registry: None }` →
    /// `RegistryKey::NPM`; `Npm { registry: Some(url) }` → whichever `[registries.*]` block
    /// declares that URL, resolved by `callisto-graph::config` (§G.5.4). Plan entries carry
    /// the key, not a serialised `PublishTarget` — see §M.12.2's SPEC DECISION for why.
    pub fn registry_key(&self) -> Option<RegistryKey>;
    pub fn ecosystem(&self) -> Option<Ecosystem>;
}

/// §5.1.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReleaseTrigger {
    /// Only bumps when a changeset names it. The default, and the only variant with working
    /// inference before v0.2 (§17, §Q5.2).
    #[default]
    Changeset,
    /// Infers from conventional commits when no changeset names it (§7.1). Needs
    /// `callisto-conventional`, v0.2.
    Auto,
}
```

### M.4 Versions — `version.rs`

#### M.4.1 `Version` is grammar-tagged

> `[SPEC DECISION, not in 00-design.md: `Version` is a grammar-tagged struct, not a newtype
> over `semver::Version`.]` §7.7 is explicit that "every place this design currently reads as
> SemVer-specific is a trait boundary" and that Maven's comparator and PEP 440 are different
> grammars, not different renderings — but §5.1's `DepSpec::Exact(Version)` and
> `Manifest::current_version() -> Version` both name a single *concrete* `Version` type. A bare
> `semver::Version` alias would make §7.7's claim false at the type level on day one, and make
> every demand-gated ecosystem a breaking change to this crate's public types rather than the
> "bounded work" P4 promises. A fully generic `Version<G>` would infect every signature in
> §M.12's contract types with a parameter that has exactly one inhabitant in committed scope. A
> grammar tag on one concrete struct is the shape that keeps §7.7 honest at zero cost to
> today's call sites, and it buys a total, checked answer to "can these two versions be
> compared?" (§M.4.2).

```rust
/// A parsed version, tagged with the grammar it was parsed under. §7.7, P4.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Version {
    grammar: VersionGrammar,
    /// The exact string this version was parsed from, preserved so a write-back can be
    /// byte-identical when nothing changed (§CM.4.1's format-preservation posture applied to
    /// values, not just files). `render()` returns this verbatim.
    raw: String,
    /// The parsed form, kept alongside `raw` so comparison and component access are cheap and
    /// `render()` never has to reconstruct a string. For `VersionGrammar::SemVer` this wraps a
    /// `semver::Version`; a future non-SemVer grammar adds its own payload variant rather than
    /// reinterpreting this one. Private — `semver::Version` never appears in a public
    /// signature (§M.1.1).
    parsed: ParsedVersion,
}

/// §7.7. `SemVer` is the only grammar with an implementation in the committed v0.1–v0.4
/// scope; the rest are declared so `Ecosystem::version_grammar` is total.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum VersionGrammar {
    SemVer,
    /// PEP 440 (§7.7) — declared, not implemented.
    Pep440,
    /// Maven's qualifier-ordering comparator (§7.7) — declared, not implemented.
    Maven,
}

impl Version {
    pub fn parse(raw: &str, grammar: VersionGrammar) -> Result<Self, VersionParseError>;
    pub fn grammar(&self) -> VersionGrammar;
    /// The original string. `render()` and the input to `parse` round-trip byte-identically
    /// for any version this crate produced by parsing; a version produced by `bump_version`
    /// (§F.6) renders canonically.
    pub fn render(&self) -> &str;

    /// SemVer component access. **Defined only for `VersionGrammar::SemVer`** — `None` for any
    /// other grammar, rather than a guess. Used by the pre-major gate (§C.4) and by
    /// `next_major` in `rewrite_spec`'s `>=A <B` shape (§G.7.7).
    pub fn major(&self) -> Option<u64>;
    pub fn minor(&self) -> Option<u64>;
    pub fn patch(&self) -> Option<u64>;
    pub fn is_prerelease(&self) -> bool;

    /// Grammar-aware ordering. `Err(GrammarMismatch)` when the two versions were parsed
    /// under different grammars — see §M.4.2.
    pub fn compare(&self, other: &Version) -> Result<std::cmp::Ordering, GrammarMismatch>;

    /// Convenience for the very common "both operands are SemVer" case; `None` on mismatch.
    pub fn partial_compare(&self, other: &Version) -> Option<std::cmp::Ordering>;
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("`{raw}` is not a valid {grammar:?} version: {message}")]
pub struct VersionParseError {
    pub raw: String,
    pub grammar: VersionGrammar,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("cannot compare a {left:?} version with a {right:?} version")]
pub struct GrammarMismatch {
    pub left: VersionGrammar,
    pub right: VersionGrammar,
}
```

#### M.4.2 Serde, and the deliberate absence of `Ord`

> `[SPEC DECISION, not in 00-design.md: `Version`'s `Deserialize` parses under
> `VersionGrammar::SemVer`; a non-SemVer ecosystem requires adding a grammar discriminator to
> the wire format *and* bumping `SCHEMA_VERSION`.]` §12.5's contract renders versions as plain
> JSON strings (`"1.3.0"` in §9.2's worked plan), which carries no grammar. Since every
> committed ecosystem is SemVer (§2.2, §7.7), assuming SemVer on the wire is correct today and
> the schema-version bump is the mechanism §13 invariant 14 already provides for the day it
> stops being. The alternative — serialising an object `{grammar, raw}` — would break every
> `jq -r '.rustCrates[].version'` consumer §9.3 demonstrates, in exchange for generality no
> committed milestone uses. The cost is named here so it is a known, schema-versioned
> migration rather than a surprise.

```rust
impl Serialize for Version {   /* serialises as `render()` — a plain string */ }
impl<'de> Deserialize<'de> for Version {   /* parses under VersionGrammar::SemVer */ }
```

> `[SPEC DECISION, not in 00-design.md: `Ord`/`PartialOrd` are deliberately **not**
> implemented for `Version`.]` A derived or hand-written total order would have to answer
> "is this PEP 440 version greater than that Maven version," which has no correct answer, and
> `Ord` has no failure channel: `BTreeMap<Version, _>` and `.max()` over a mixed-grammar
> collection would silently produce a wrong answer.
> `compare`/`partial_compare` make the caller face the question. `compare` returns `Result`,
> and every call site that needs a maximum handles the mismatch explicitly rather than
> silently ordering by struct field order — three shapes, not one, since "explicitly" means
> different things depending on whether an edge, a group, or neither is in scope:
> `select_last_tag` (§M.9.4) is single-grammar *by construction* (one `grammar: VersionGrammar`
> parameter covers every candidate), so a mismatch cannot arise there at all — a per-candidate
> parse failure is a `VersionParseError` surfaced via `LastTagSelection::skipped`, not a
> `compare()` call; §G.7.4's cascade (an edge in scope) produces `GraphError::GrammarMismatch
> { from, to, source }`; §G.6.7's linked-group union and §G.8.2/§G.8.3's fixed-group
> alignment/aligned-base fallback (a group, no edge) produce
> `GraphError::GroupGrammarMismatch { group, members }` instead, since fabricating an edge's
> `from`/`to` for a group-internal comparison would misdescribe what happened (§G.12).
> `PartialEq`/`Eq` *are* derived and are field-level (including
> prerelease and build metadata), which is stronger than SemVer precedence equality — that
> is the equality §F.9's `bump_version` golden table needs. SemVer precedence treats `1.2.3`
> and `1.2.3+abc` as equal, which would let a build-metadata bug pass a fixture.

#### M.4.3 `VersionReq`

```rust
/// A parsed version requirement, grammar-tagged for the same reason `Version` is. Constructed
/// only by `callisto-manifests`' per-ecosystem parsers (§CM.4.2, §CM.5.2), which is where the
/// grammar differences actually live — Cargo's comma-AND clauses and npm's hyphen ranges and
/// `||` OR-groups are not the same language.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionReq {
    grammar: VersionGrammar,
    ecosystem: Ecosystem,
    req: semver::VersionReq,
    raw: String,
}

impl VersionReq {
    pub fn parse(raw: &str, ecosystem: Ecosystem) -> Result<Self, VersionParseError>;
    pub fn render(&self) -> &str;
    pub fn ecosystem(&self) -> Ecosystem;
    /// Does this requirement admit `v`? `Err` on a grammar mismatch, never a silent `false`
    /// — a cross-ecosystem edge that got this far is `GraphError::GrammarMismatch` (§G.12).
    pub fn matches(&self, v: &Version) -> Result<bool, GrammarMismatch>;
}
```

npm's and Cargo's requirement grammars are *both* `VersionGrammar::SemVer` at the **version**
level but differ at the **requirement** level (npm hyphen ranges and `||` OR-groups; Cargo's
comma-AND clauses; opposite bare-string defaults — npm bare is exact, Cargo bare is caret).
Those are two different axes, and the requirement dialect is the one that needs an `Ecosystem`
rather than a second `VersionGrammar` variant — which is why `parse` takes an `Ecosystem` while
`Version::parse` takes a `VersionGrammar`. `render()` returns the preserved original string;
re-*rendering* a bumped requirement is `callisto-manifests::round_trip`'s job, not this type's
(§CM.3, §11.2 R6).

#### M.4.4 `SCHEMA_VERSION` lives with the report types

`SCHEMA_VERSION` is declared in `report.rs` and documented in §M.12.1; it is named here only
because §13 invariant 14 ties it to the same "versions are a contract" concern this section
governs.

#### M.4.5 Why `Version` living here fixes the `callisto-format → callisto-model` edge

> `[SPEC DECISION, resolving a conflict inside 00-design.md: `Severity`, `Version`,
> `VersionGrammar`, and `VersionReq` are canonically defined in `callisto-model`; the crate
> dependency edge is `callisto-format → callisto-model`, and `callisto-model` depends on no
> `callisto-*` crate at all.]` §15's crate-content bullet lists `Severity` under
> `callisto-model`; §15's `callisto-format` bullet says "zero deps on workspace/moon concepts,"
> which is a statement about *concepts*, not about `callisto-*` crates. The two readings only
> conflict if "zero deps" is read maximally, and reading it that way costs more than it saves:
>
> - `bump_version` (§F.6.2) applies a severity to a version. If it took `semver::Version` while
>   the workspace's version of record is `callisto_model::Version` (§M.4.1), every call site in
>   `callisto-graph` would convert — and a conversion that can fail (grammar) would be threaded
>   through a function §13 invariant 2 says must be exact and unconditional.
> - `PreState::initialVersions` (§F.7) holds versions of record. Same argument.
> - Two definitions of `Severity` in one workspace is exactly the two-independently-evolved-
>   paths failure the decision doc cites `#2207` for, applied to a four-variant enum — the same
>   "one function, one concept" rule §13 invariant 25 imposes on tag resolution.
>
> The property `callisto-format`'s zero-dependency framing was actually protecting — that a
> team wanting a Rust changesets-format implementation without buying into callisto's
> graph/cascade/manifest opinions can depend on `callisto-format` alone — survives intact,
> because `callisto-model` is itself MIT/Apache (§16), has zero `callisto-*` dependencies
> (§M.1.1), and contains no coordination logic. **The primitive worth spreading is a two-crate
> primitive, not a one-crate primitive.** Both edges (`callisto-format → callisto-model`, and
> `callisto-model → nothing`) are CI-enforced by the same `cargo metadata` audit §13 invariant
> 26 uses for the moon-free rule (§G.1.7). §F.2 and §11.2 R1 record the corresponding reversal
> in `callisto-format`'s own draft, which had proposed the opposite direction.

### M.5 Severity — `severity.rs`

`Severity` is declared here, not in `callisto-format`, per §M.4.5. `callisto-format`
re-exports it (`pub use callisto_model::Severity;`) so a consumer that wants only the
changesets primitive still gets the enum from one import.

```rust
/// §5.1, §6.1. A changeset's declared severity for one named package, **and** §7.4's internal
/// cascade outcome for an out-of-range dev-dependency ("spec rewrite only, no version bump").
/// §6.1 is explicit that these two things share one enum value and that only the file-format
/// one is ever persisted to disk as a changeset — `callisto-format` is responsible for the
/// file-format half only; the cascade-outcome usage lives entirely in `callisto-graph` and
/// never round-trips through a changeset file.
///
/// **Variant order is deliberate, not alphabetical.** The derived `Ord` below is the
/// aggregation-by-max lattice §7.1 relies on: `None < Patch < Minor < Major`, so `.max()`
/// over a package's severities from multiple changesets, from a fixed-group union, and from
/// inference all do the right thing with one operator.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    None,
    Patch,
    Minor,
    Major,
}

impl Severity {
    /// All four variants in ascending order — for exhaustive fixture tables and CLI help
    /// text, so no second hand-maintained list can drift from the enum.
    pub const ALL: [Severity; 4] =
        [Severity::None, Severity::Patch, Severity::Minor, Severity::Major];
}

/// §6.1: "case-insensitive read, lowercase write." `FromStr` is the read half, `Display` the
/// write half. The asymmetry is the spec, not a bug to unify.
impl std::str::FromStr for Severity {
    type Err = SeverityParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "major" => Ok(Severity::Major),
            "minor" => Ok(Severity::Minor),
            "patch" => Ok(Severity::Patch),
            "none"  => Ok(Severity::None),
            _ => Err(SeverityParseError { found: s.to_string() }),
        }
    }
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Severity::Major => "major",
            Severity::Minor => "minor",
            Severity::Patch => "patch",
            Severity::None  => "none",
        })
    }
}

/// The token read where `major | minor | patch | none` (any case) was expected.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("invalid severity {found:?}: expected one of \"major\", \"minor\", \"patch\", \"none\" (case-insensitive)")]
pub struct SeverityParseError {
    pub found: String,
}
```

`FromStr`/`Display`/`SeverityParseError` live here rather than in `callisto-format` because
Rust's orphan rule puts an impl with the type; the doc comment records that the *rule* those
impls encode is §6.1's, a file-format rule, so a future reader does not mistake the
placement for the concept's home.

### M.6 Package and manifests — `package.rs`

#### M.6.1 `Package`

> `[SPEC DECISION, not in 00-design.md: `Package` keeps exactly §5.1's six fields;
> `pre-major-inference` and group membership are **not** fields on it.]` Both are resolved
> config, and §14 attaches them to `[[package-set]]`/`[[package]]`/`[[fixed-group]]`/
> `[[linked-group]]` blocks, not to a package. Group membership in particular is a *relation
> over* packages — putting a `group: Option<GroupName>` on `Package` would make "is this
> package in a group" answerable two ways (field vs. `GroupTable` lookup), which is the
> two-code-paths-for-one-concept shape §13 invariant 25 and `#2207` warn about. `ResolvedConfig`
> (§G.5.4) owns both.

> `[SPEC DECISION, not in 00-design.md: napi platform packages are `ManifestRole::Platform`
> manifests of the main `Package`, not separate `Package` values.]` §5.2 is authoritative
> here — "Case E (napi single package): one package, canonical manifests for the crate and
> the main JS package, N `Platform` manifests." §14's `[[fixed-group]] members` list is the
> *naming* surface for those platforms (§G.5.5's `GroupMember` two-variant type), not evidence
> that each is its own `Package`. This is what makes §13 invariant 20 ("platform manifests
> never receive an independent git tag") structural rather than a filter someone must remember
> to apply: `is_release_point` below is a property of a `Package`, and platform manifests are
> not `Package`s.

```rust
/// §5.1. The resolved product of discovery + config; constructed by `callisto-graph`'s graph
/// construction (§G.4), never by a user of this crate directly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Package {
    pub id: PackageId,
    /// At least one `Canonical` entry (`ModelError::NoCanonicalManifest` otherwise). A Case D
    /// package has two; a napi package has two canonical plus N `Platform` plus its lockfiles.
    pub manifests: Vec<ManifestDecl>,
    /// Declared by §5.1, which gives the field's type but not `None`'s meaning — 00-design.md
    /// §5 ends at §5.4 and has no further subsection to cite. **This spec supplies the
    /// meaning, in §CL.6's SPEC DECISION: `None` is an opt-out.** No `CHANGELOG.md` is written
    /// for this package at all; it does not mean "resolve a default path." `callisto init`
    /// populates a real path for every package it discovers (§G.11), so `None` is reachable
    /// only when a user has explicitly edited it out. `plan-publish` omits
    /// `ReleaseEntry.changelog_section` for such a package (§CL.7.1).
    pub changelog: Option<PathBuf>,
    pub release_trigger: ReleaseTrigger,
    pub publish_to: Vec<PublishTarget>,
    /// `None` uses §9.1's default template, `{name}@{version}`, with `{name}` substituted from
    /// `id.display_name()`. Resolved once, by `TagIndex::build` (§G.9.2).
    pub tag_template: Option<TagTemplate>,
}

impl Package {
    /// Every `ManifestRole::Canonical` entry.
    pub fn canonical_manifests(&self) -> impl Iterator<Item = &ManifestDecl>;
    /// Every `ManifestRole::Platform` entry (§5.2, §7.6 step 4).
    pub fn platform_manifests(&self) -> impl Iterator<Item = &ManifestDecl>;
    /// Every `ManifestRole::Lockfile` entry — named for `--refresh-lockfiles` bookkeeping
    /// (§M.12.3's `LockfileRefreshResult.filename`), never opened as a `Manifest` (§CM.2.4).
    pub fn lockfiles(&self) -> impl Iterator<Item = &ManifestDecl>;

    /// The set of `VersionGrammar`s across this package's canonical manifests. More than one
    /// distinct value is `ModelError::MixedVersionGrammars` — a package whose version of
    /// record has no single grammar cannot be bumped coherently.
    pub fn version_grammar(&self) -> Result<VersionGrammar, ModelError>;

    /// **§13 invariant 20, made structural.** `true` when this package is a real release
    /// point — i.e. it publishes to at least one target that is not `PublishTarget::None`.
    /// `PublishPlan.releases[]` (§M.12.2) is built by filtering on this, so there is no
    /// separate "remember to exclude platform packages" step: platform manifests belong to a
    /// `Package`, they are not one.
    pub fn is_release_point(&self) -> bool;

    /// Case D (§2.1): ≥2 canonical manifests spanning ≥2 ecosystems, one version of record.
    pub fn is_dual_published(&self) -> bool;
}
```

#### M.6.2 `ManifestDecl`, `ManifestRole`, `ManifestFormat`

```rust
/// §5.2. The *declared* shape of one manifest file — a path/role/format triple. Distinct from
/// the `Manifest` trait (§CM.1), which is the runtime file handle constructed from one of
/// these; §5.1's own note records the rename that keeps the two apart.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestDecl {
    /// Workspace-root-relative (§M.1.3).
    pub path: PathBuf,
    pub role: ManifestRole,
    pub format: ManifestFormat,
}

impl ManifestDecl {
    /// Validating constructor. Rejects an absolute or non-UTF-8 path (§M.1.3) and a
    /// role/format pair that cannot occur — `ModelError::InvalidRoleForFormat` — e.g.
    /// `Platform` on a `CargoToml` (napi platform packages are npm packages; the crate is the
    /// canonical Rust side, never a platform of itself) or `Canonical` on a lockfile format.
    pub fn new(path: impl AsRef<Path>, role: ManifestRole, format: ManifestFormat)
        -> Result<Self, ModelError>;
    pub fn ecosystem(&self) -> Ecosystem;
}

/// §5.2 — the axis §5.2 calls callisto's key differentiator over knope's model.
///
/// `PartialOrd, Ord` added for `GroupMember` (§G.5.5), which embeds a `ManifestRole` and
/// must derive `Ord` itself — `GroupDef::members` is documented sorted, and every field of
/// `Platform` (`String`, `String`, `Option<String>`) supports it, so this costs nothing.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ManifestRole {
    /// Holds the version of record; ≥1 per package.
    Canonical,
    /// napi-style; inherits the parent package's version unconditionally at §7.6 step 4.
    Platform { platform: String, arch: String, abi: Option<String> },
    /// Regenerated by subprocess (§7.6 step 9), never version-written. `callisto-manifests`
    /// refuses to open one as a `Manifest` at all (§CM.2.4).
    Lockfile,
}

/// §5.2. Committed formats first; the rest are declared-only (§CM.7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum ManifestFormat {
    CargoToml,
    PackageJson,
    // demand-gated:
    PyprojectToml,
    SetupCfg,               // read-only; write target is pyproject.toml (§7.8)
    GoMod,
    PomXml,
    GradleVersionCatalog,
    SettingsGradle,
    VersionSbt,
    DenoJson,
    // lockfiles:
    CargoLock,
    PackageLockJson,
    PnpmLockYaml,
    YarnLock,
}

impl ManifestFormat {
    pub fn ecosystem(&self) -> Ecosystem;
    pub fn is_lockfile(&self) -> bool;
    /// `false` for `SetupCfg`, every lockfile, and the imperative Gradle/sbt scripts §7.8
    /// names as unwritable. `callisto-manifests::open` turns this into
    /// `ManifestError::ReadOnlyFormat` rather than a silent skip (§7.8's "hard error, not
    /// silent skip" posture).
    pub fn is_writable(&self) -> bool;
    /// The conventional basename — `"Cargo.toml"`, `"package.json"`, … Used by
    /// `IgnoreWalkLocator`'s discovery (§G.2.2) and by `init`'s narration (§G.11), so neither
    /// carries its own hard-coded filename table.
    pub fn file_name(&self) -> &'static str;
}
```

`YarnLock` is added to §5.2's enum because §CM.5.4's `WorkspaceKind` detection reads
`yarn.lock`'s presence, and a lockfile a package can name in `manifests` needs a
`ManifestFormat` to name it with.

### M.7 Dependencies — `dependency.rs`

#### M.7.1 `DepKind`, `DepSpec`, `WorkspaceKind`

```rust
/// §5.1, §7.2. An edge's dependency kind, as declared by the manifest it was read from.
///
/// `Optional` is Cargo's `optional = true` flag on a `[dependencies]` entry **and** npm's
/// `optionalDependencies` section — the two are the same concept for cascade purposes (§7.4's
/// first three rows treat `Runtime | Optional | Build` identically) and are kept separate from
/// `Runtime` because §7.5's napi coordination pins platform packages through
/// `optionalDependencies` specifically.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DepKind { Runtime, Dev, Peer, Optional, Build }

/// §7.3. Parsed once at graph construction, keeping the original string, so a rewrite is
/// lossless or is refused (§13 inv. 15).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum DepSpec {
    /// npm's bare `"1.2.3"` — an exact-match requirement in npm's grammar.
    Exact(Version),
    /// Parsed requirement plus the original string, for lossless round-trip.
    Range(VersionReq, String),
    /// pnpm/Yarn `workspace:*` etc. Never bumped — the workspace tool resolves it (§7.3).
    Workspace(WorkspaceKind),
    /// pnpm catalog reference (`catalog:` / `catalog:<name>`). Never rewritten (§M.7.2).
    Catalog(Option<String>),
    /// Cargo's bare `"1.2.3"`, which is semantically caret. A separate variant from `Exact`
    /// because the identical literal string means a *different requirement* in the two
    /// ecosystems (§CM.5.2) — one variant would silently launder that difference.
    CargoBare(Version),
    /// Anything unrecognised — git/path/alias specs, multi-clause ranges the parser declines
    /// to model. Left untouched (§7.3).
    Opaque(String),
}

impl DepSpec {
    /// The original text, exactly as it appeared in the manifest — for diagnostics and as
    /// `round_trip`'s textual input (§CM.3). `Workspace`/`Catalog` render their protocol
    /// string; `Exact`/`CargoBare` render the version.
    pub fn render(&self) -> String;
}

/// §5.1, §7.3. Which npm-ecosystem tool's conventions govern this workspace. Resolved once
/// per invocation from lockfile presence (§CM.5.4), never per package.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceKind { Pnpm, Yarn, Npm }
```

Two per-ecosystem absences, handled symmetrically by one rule (§CM.4.2, §CM.5.2): Cargo has no
peer-dependency concept, and npm has no build-dependency section. In both cases
`update_dependency_spec` with the absent kind returns `ManifestError::DependencyNotFound` —
"no such section, therefore no such entry" — rather than a format-specific special case.

#### M.7.2 `Coverage`

```rust
/// §7.4's "spec covers new version" column, as a three-valued answer rather than a bool.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Coverage {
    /// The declared spec still admits the new version — §7.4's ✓ rows.
    Covers,
    /// It does not — §7.4's ✗ rows.
    DoesNotCover,
    /// The question is not answerable from the spec alone.
    Unknown,
}
```

> `[SPEC DECISION, not in 00-design.md: `DepSpec::Catalog` is `Coverage::Unknown` and is
> never rewritten.]` §7.3 lists `Catalog` without saying how cascade treats it. A pnpm catalog
> entry lives in `pnpm-workspace.yaml`, not in the dependent's `package.json`, so the spec
> string in the manifest carries no version information at all — answering `Covers` would be
> a guess and answering `DoesNotCover` would trigger a rewrite of a string that is not a
> version requirement. It routes through §13 invariant 15's warn-and-leave-alone path with
> `DiagnosticCode::CatalogSpecNotRewritten`, so a workspace that bumps a catalogued member
> gets told, once, that callisto did not touch the catalog.

`Coverage` is computed by `callisto-graph` (§G.7.3), from a `DepSpec` plus a candidate
`Version`. `callisto-manifests` never asks the coverage question — it only parses spec
strings and rewrites them (§CM.3).

#### M.7.3 `DependencyEntry` and `DepEdge`

```rust
/// §5.1. One raw dependency record as read off a single manifest, **pre-resolution** to a
/// `PackageId`. Yielded by `Manifest::iter_dependencies` (§CM.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DependencyEntry {
    pub name: String,
    /// The declaring **section's** kind, always the *member's* own section — `Dev` for an
    /// entry under `[dev-dependencies]`, even when `inherited` is `true` (§18 Q2's
    /// `foo.workspace = true`), because the workspace root's `[workspace.dependencies]` table
    /// has no sections and therefore contributes no kind of its own.
    pub kind: DepKind,
    /// For an inherited entry this is the **resolved** spec from the workspace root, not a
    /// placeholder — §CM.4.2's "a caller should never have to know inheritance was involved
    /// to read the effective spec."
    pub spec: DepSpec,
    /// `true` iff this entry's *value* comes from somewhere other than this manifest — at
    /// v0.1 that is exactly Cargo's `foo.workspace = true` (§18 Q2), resolved out of the
    /// workspace root's `[workspace.dependencies]`. Always `false` for `package.json`, which
    /// has no inheritance mechanism.
    ///
    /// This is the flag graph construction reads to decide **which file a spec rewrite must
    /// edit** (§G.4.4): a rewrite for an inherited entry lands in the workspace root, once,
    /// not in each member as a local override that would silently shadow the workspace value.
    pub inherited: bool,
}

/// §5.1. One resolved, graph-level edge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DepEdge {
    pub from: PackageId,
    pub to: PackageId,
    pub kind: DepKind,
    pub spec: DepSpec,
    /// The manifest that *declares* this dependency — the file a spec rewrite must edit. For
    /// an inherited entry (below) this is the workspace root's `Cargo.toml`, not `from`'s own
    /// manifest.
    pub from_manifest: PathBuf,
    /// Carried through from `DependencyEntry::inherited` (§M.7.3 above). Read only by the
    /// rewrite worklist, which needs it to choose between `Manifest::update_dependency_spec`
    /// and `WorkspaceCargoResolver::write_dependency` (§G.7.3's `DepWriteTarget`) — a bare
    /// `from_manifest` cannot decide that, because a Cargo workspace root may be both a
    /// package and the holder of `[workspace.dependencies]` (§G.2.2).
    pub inherited: bool,
}
```

> `[SPEC DECISION, not in 00-design.md: `DepEdge` gains a `from_manifest: PathBuf` field
> beyond §5.1's four, and graph construction emits one edge per (declaring manifest,
> dependency entry) pair rather than one per (from, to, kind) triple.]` Two committed cases
> make the declaring file non-derivable from `from`: a Case D package declares dependencies
> in both its `Cargo.toml` and its `package.json`, and Cargo's `foo.workspace = true`
> inheritance (§18 Q2) means a member's edge is *declared* in the workspace root's
> `Cargo.toml`, not in the member's. §7.6 step 6 has to know which file to edit, and
> re-deriving it at write time would be a second resolution path for a fact graph
> construction already knew — §13 invariant 25's failure shape. §G.4.4 records the
> corresponding de-duplication rule for rewrites.

> `[SPEC DECISION, not in 00-design.md: workspace inheritance is signalled by a
> `DependencyEntry::inherited: bool` field, **not** by a `DepSpec` variant and not by a
> `DepSpec` method.]` §18 Q2 requires graph construction to detect an inherited dependency
> (so it can record the workspace root as the rewrite target, §G.4.4), but §CM.4.2 also
> requires `iter_dependencies` to yield the *resolved* spec so that no reader has to know
> inheritance happened. Those two requirements cannot both be met by anything living inside
> `DepSpec`: the resolved spec of an inherited `serde = { workspace = true }` **is** the
> root's `"1.0"`, byte for byte, and the same `DepSpec` value must be produced for a member
> that spells that dependency out locally. Provenance is a property of the *record*, not of
> the requirement, so it goes on `DependencyEntry`. Two further reasons this is the smaller
> change: `DepSpec` is a serialized contract type (`#[serde(tag = "kind")]`, §M.7.1), so a
> variant would be a wire-format change; and every `DepSpec` consumer matches exhaustively
> (`coverage` §G.7.2, `round_trip` §CM.3, `rewrite_spec` §G.7.7), so a variant would force
> three unrelated functions to grow an arm for a case none of them can act on. An earlier
> draft of §G.4.4 called a nonexistent `entry.spec.is_workspace_inherited()`; this field is
> what that call site actually reads (§11.2 R21).

### M.8 Discovery types — `discovery.rs`

```rust
/// §15. One located project root, from either `IgnoreWalkLocator` or `MoonProjectLocator`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectRoot {
    pub id: PackageId,
    /// Workspace-root-relative (§M.1.3).
    pub path: PathBuf,
    pub ecosystem: Ecosystem,
}

/// §15, decision-doc change 2. moon's declared edge, mapped into callisto-owned vocabulary.
/// **Never** moon's own `DependencyScope` — reusing that type would either leak a moon type
/// into the moon-agnostic core (violating §0.1 rule 1) or silently launder the fact that the
/// mapping is lossy (§MO.4.4's table is where the lossiness is written down).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeclaredEdge {
    pub from: PackageId,
    pub to: PackageId,
    pub kind: DeclaredEdgeKind,
    /// moon's own provenance field, carried through for a human-readable diagnostic only.
    /// Never compared (§G.4.6).
    pub via: Option<String>,
}

/// §15 — deliberately named and shaped after moon's `DependencyScope`, not callisto's
/// `DepKind`. moon has `Root` and no `Optional`; moon's `Production` does not cleanly split
/// into callisto's `Runtime` + `Optional`, which is exactly the distinction §7.5's napi
/// `optionalDependencies` pattern depends on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeclaredEdgeKind { Build, Development, Peer, Production, Root }
```

> `[SPEC DECISION, not in 00-design.md: `ProjectLocator::projects()` emits one `ProjectRoot`
> per (root path, ecosystem) pair, so a Case D root yields two entries at the same path.]`
> §15 pins `ProjectRoot`'s three fields, and `ecosystem: Ecosystem` is singular — there is no
> shape in which one `ProjectRoot` can describe a co-located `Cargo.toml` + `package.json`.
> Collapsing the pair into one `Package` therefore happens during graph construction (§G.4.3),
> which is also where §5.4(b)'s "both sides must resolve to the same `PackageId`" rule can be
> checked and `GraphError::SplitIdentity` raised. Keeping the collapse out of the locator also
> keeps `MoonProjectLocator` from needing a Case D concept at all (§MO.4.2).

### M.9 Tags — `tag.rs`

#### M.9.1 `TagTemplate`

```rust
/// §9.1. A validated tag template: literal text with exactly one `{version}` placeholder.
/// There is no `{name}` placeholder — a template is scoped to one package by construction, so
/// the name is already fixed (§9.1). The **default**, when `Package::tag_template` is `None`,
/// is `{name}@{version}` with `{name}` substituted once from `PackageId::display_name` — that
/// substitution happens at template *construction*, so by the time a `TagTemplate` exists the
/// name is literal text and `{version}` is the only placeholder left.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TagTemplate {
    prefix: String,   // literal text before {version}
    suffix: String,   // literal text after {version}
}

impl TagTemplate {
    /// Validated at **config-load time** (§G.12's `ConfigError::Tag`), never lazily: a bad
    /// template's failure mode is a glob that matches every tag in the repository, which is
    /// silent and destructive rather than loud.
    pub fn parse(raw: &str) -> Result<Self, TagTemplateError>;

    /// The §9.1 default for a package with no configured template.
    pub fn default_for(id: &PackageId) -> Self;

    /// Render a concrete tag name. This is the **one function** §13 invariant 25 requires,
    /// used identically by §7.5's group forcing, `PublishPlan.releases[].tagName` (§M.12.2),
    /// and `last_tag_for`'s search (§M.9.3). No call site anywhere in this workspace
    /// concatenates a name, an `@`, and a version — §G.9.2 records that a grep for `'@'`
    /// adjacent to a version is a review smell for exactly this reason.
    pub fn render(&self, version: &Version) -> TagName;

    /// The literal git glob derived from the template's non-placeholder text —
    /// `foo@{version}` → `foo@*`. Glob metacharacters in the literal parts are rejected at
    /// parse time (`GlobMetacharacterInLiteral`, §M.9.5), so this is a pure
    /// `format!("{prefix}*{suffix}")`.
    pub fn glob(&self) -> String;

    /// Given a tag name that matched `glob()`, return the substring at the placeholder's
    /// position, or `None` when the tag does not actually bracket the prefix/suffix (a glob
    /// can over-match; this is the exact check).
    ///
    /// **Never attempts to invert the template as a whole** (§9.1): an arbitrary
    /// `{version}`-interpolated template is not generally invertible, so extraction is
    /// positional and *parsing* is what discriminates (§M.9.3 step 4), not the glob.
    pub fn extract_version_str<'a>(&self, tag: &'a str) -> Option<&'a str>;

    pub fn as_str(&self) -> String;   // round-trips `parse`
}

/// A rendered git tag name. A newtype so that a `String` holding something else can never be
/// passed where a tag is expected, so `CreatedTag`/`ReleaseEntry`/`LastTag` all agree, and so
/// §13 invariant 25's "exactly one function" claim is checkable by grepping for constructions
/// of this type.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TagName(pub String);

impl TagName { pub fn as_str(&self) -> &str; }
impl std::fmt::Display for TagName { /* … */ }
```

> `[SPEC DECISION, not in 00-design.md: a template with no literal text around `{version}`
> (i.e. the template `"{version}"`) is rejected at config-load time with
> `TagTemplateError::NoLiteralAnchor`.]` §9.1 requires `{version}` exactly once and derives a
> glob from the surrounding literals; it does not say what happens when there are none. The
> derived glob would be `*`, which matches every tag in the repository — every sibling
> package's tags would become candidates, and the parse step would then pick the numerically
> highest across the whole workspace. That is a silent cross-package data corruption, so it is
> refused at the earliest point it can be (P5).

#### M.9.2 The one tag-resolution path

`TagTemplate::render` is the single function §13 invariant 25 names. Its three callers —
group forcing (§G.8.3), `PublishPlan.releases[].tagName` (§M.12.2), and `last_tag_for`
(§M.9.3/§G.9.1) — all obtain the template the same way, from `TagIndex` (§G.9.2), which
resolves `Package::tag_template` or `TagTemplate::default_for(&package.id)` once per package
per run. `#2207` is the failure this arrangement exists to preclude: two independently
evolved identity/tag paths that silently disagree when a flag perturbs one of them.

#### M.9.3 `last_tag_for`, the algorithm

Steps 1–3 need `git` and therefore live in `callisto-graph` (§G.9.1); steps 4–6 are pure and
live here (§M.9.4). §13 invariant 25 is satisfied jointly by the two halves — there is no
second glob-and-extract path anywhere in the workspace.

```
 1. resolve the package's TagTemplate (Package::tag_template, else TagTemplate::default_for).
 2. glob ← template.glob().
 3. candidates ← `git tag --list <glob>`, one tag per line, blank lines dropped.
    A non-zero exit here IS a failure: `git tag --list` with no matches exits 0 with empty
    output, so non-zero means the repository is unreadable, not that there are no tags.
    (This is a deliberate local exception to CommandRunner's general "non-zero is not an
    error" contract, §M.10 — noted at the call site, §G.9.1.)
 4. for each candidate line:
      a. template.extract_version_str(line)  — None ⇒ the glob over-matched; skip SILENTLY,
         since a glob is a superset by construction and a non-match is not a diagnostic.
      b. Version::parse(extracted, grammar)  — Err ⇒ emit
         DiagnosticCode::TagGlobNonVersionMatch and skip. Never a silent drop: a workspace
         where this fires constantly has a template collision worth telling the human about.
    # The parse step is a load-bearing filter, not a formality. Worked example: package
    # `foo` with the default template globs `foo@*`; a sibling `foo-bar@1.2.3` does not
    # match that glob at all, so 4a drops it silently. But a package `foo` with template
    # `foo-{version}` globs `foo-*`, which DOES match `foo-bar-1.2.3`, whose extracted
    # middle `bar-1.2.3` fails to parse — 4b, with a diagnostic. `foo@nightly` under the
    # default template is the same case.
 5. pick the maximum by `Version::compare` (grammar-aware; a grammar mismatch among
    candidates is impossible in practice since all candidates come from one package's own
    grammar, and is `Err` if it ever occurs). On a precedence tie — SemVer ignores build
    metadata for ordering, so `1.2.3` and `1.2.3+abc` tie — pick the candidate whose raw tag
    string sorts last byte-wise, so the result is deterministic and fixturable rather than
    dependent on `git tag --list` output order.
 6. return Some(LastTag { name, version }) or None when candidates is empty.
```

Prereleases are **included**, not filtered: a published prerelease is a real release point
and gets a real tag (§9.1, §9.3), and both §6.3's "changes since that package's last-release
tag" and §7.1's commits-since-tag inference window want the most recent one. §8's pre-mode
does not use `last_tag_for` for bump math at all — it bumps from `initialVersions` — so no
prerelease filter is needed anywhere.

#### M.9.4 Model-side types and the pure selection function

```rust
/// The resolved most-recent release tag for one package.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LastTag {
    pub name: TagName,
    pub version: Version,
}

/// The outcome of steps 4–6 above, as a pure function over an already-obtained candidate list.
#[derive(Clone, Debug, Default)]
pub struct LastTagSelection {
    /// `None` when no candidate survived extraction and parsing.
    pub chosen: Option<LastTag>,
    /// One entry per tag that matched the glob but whose placeholder region was not a valid
    /// version. Surfaced as warnings, never silently dropped — a workspace where these fire
    /// constantly has a template collision worth telling the human about.
    pub skipped: Vec<Diagnostic>,
}

/// Steps 4–6 of §M.9.3, with no I/O. `callisto-graph` performs steps 1–3 (which need
/// `CommandRunner`) and calls this.
///
/// Kept in `callisto-model` because it is the half of `last_tag_for` that decides *identity*,
/// and §13 invariant 25 puts identity resolution in this crate. The `git`-invoking wrapper is
/// I/O and belongs with the other I/O.
pub fn select_last_tag<'a>(
    template: &TagTemplate,
    grammar: VersionGrammar,
    candidates: impl IntoIterator<Item = &'a str>,
) -> Result<LastTagSelection, VersionParseError>;
```

> `[SPEC DECISION, not in 00-design.md: `last_tag_for` is split — the pure
> glob/extract/select half lives here as `TagTemplate::glob`,
> `TagTemplate::extract_version_str`, and `select_last_tag`; the `git tag --list`
> invocation that feeds it lives in `callisto-graph`, which owns the `CommandRunner` call
> sites.]` §17 v0.1 commits to a "stateless `last_tag_for` primitive (§9.1's glob-and-extract
> resolution)" without naming a crate, and §13 invariant 25 only requires that the
> *resolution* be one function in `callisto-model`. Splitting it keeps §M.1.2's no-I/O rule
> intact and keeps the interesting half — the part with the `#2207` failure mode in it —
> unit-testable from a literal list of tag strings with no git repository, which is what
> makes it a fixture per §12.6.

#### M.9.5 `TagTemplateError`

```rust
#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum TagTemplateError {
    #[error("tag template `{template}` contains no `{{version}}` placeholder")]
    MissingVersionPlaceholder { template: String },

    #[error("tag template `{template}` contains `{{version}}` {count} times; exactly one is \
             required")]
    MultipleVersionPlaceholders { template: String, count: usize },

    /// `{name}` lands here specifically, with a message that says so — it is the placeholder
    /// people will try (§9.1 says why it does not exist).
    #[error("tag template `{template}` contains unknown placeholder `{{{placeholder}}}`; the \
             only placeholder is `{{version}}` (the package name is fixed per package and is \
             already substituted into the default template)")]
    UnknownPlaceholder { template: String, placeholder: String },

    #[error("tag template `{template}` contains glob metacharacter `{ch}` outside the \
             `{{version}}` placeholder; the literal parts of a template become a git tag glob")]
    GlobMetacharacterInLiteral { template: String, ch: char },

    #[error("tag template `{template}` has no literal text around `{{version}}`; its tag glob \
             would be `*` and would match every tag in the repository")]
    NoLiteralAnchor { template: String },

    #[error("tag template `{template}` renders `{rendered}`, which is not a legal git ref name")]
    InvalidGitRefName { template: String, rendered: String },
}
```

Git ref legality check (applied to a rendering with a representative version): no ASCII
control characters, no space, no `~ ^ : ? * [ \`, no `..`, no leading or trailing `/`, no
`//`, no component beginning with `.`, no `.lock` suffix on any component, and not the single
character `@`. This is a subset check that rejects everything git rejects; it does not attempt
to be exactly `git check-ref-format`.

### M.10 The exec seam — `exec.rs`

```rust
/// The exec seam. §0.1 rule 2, §11, §15.
///
/// `last_tag_for` (§9.1) and `Auto`-trigger commit inference (§7.1) both need `git`. This is
/// a trait rather than a direct `std::process::Command` call specifically so that the core
/// compiles for `wasm32-wasip1` without a `#[cfg]`-gated call site (§0.1 rule 2) —
/// `std::process::Command` *compiles* under WASI and fails only at runtime through std's
/// `unsupported` shim, which is exactly the class of failure a build-only conformance check
/// misses.
///
/// Two implementations, no third anticipated:
/// - `callisto-cli`: a real subprocess (§CLI.3).
/// - `callisto-moon`: routed through moon's `exec_command` host function via `warpgate_pdk`'s
///   typed `exec`/`command_exists` bridge (§MO.5).
///
/// **The trait is deliberately dyn-compatible** — every method takes `&self` and borrowed
/// arguments and returns owned values — so a caller may take `&dyn CommandRunner`
/// (`callisto-conventional` does, §C.5) or a generic `R: CommandRunner` (`callisto-graph`
/// does, §G.11) and the two interoperate by unsized coercion. Neither form is privileged;
/// keeping dyn-compatibility is what makes that true, and it is worth preserving on any
/// future edit.
///
/// **Implementations must capture both streams; they must never inherit stdio.** §13
/// invariant 5 requires that in `--format json` mode nothing but the intended JSON reaches
/// stdout, and requires it structurally, at the spawn site. An implementation that inherits
/// stdout puts `git`'s output on callisto's stdout and corrupts the JSON contract — so the
/// rule lives on this trait's contract, not in a caller's checklist.
///
/// **A non-zero exit is not an error.** `run` returns `Ok(CommandOutput)` for any process
/// that started and finished, whatever its exit code; `git tag --list` with no matches exits
/// 0 with empty output, and `git describe` failing to find a tag is information, not a
/// failure. `CommandError` is reserved for "the command could not be run at all."
pub trait CommandRunner: Send + Sync {
    fn run(&self, program: &str, args: &[&str], cwd: &Path)
        -> Result<CommandOutput, CommandError>;
}

/// A completed subprocess. §15.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandOutput {
    /// Process exit code. `None` when the process was terminated by a signal (never
    /// observable under WASI, possible under the CLI).
    pub exit_code: Option<i32>,
    /// Captured stdout, lossily decoded as UTF-8. Every command callisto runs is `git`,
    /// whose output is text; lossy decoding keeps a stray invalid byte in a commit message
    /// from failing a whole version pass.
    pub stdout: String,
    /// Captured stderr, lossily decoded. Forwarded to callisto's own stderr by the
    /// implementation (§CLI.3), never to stdout (§13 inv. 5).
    pub stderr: String,
}

impl CommandOutput {
    pub fn success(&self) -> bool;                             // exit_code == Some(0)
    pub fn stdout_trimmed(&self) -> &str;
    pub fn stdout_lines(&self) -> impl Iterator<Item = &str>;  // non-empty lines, trimmed
}

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum CommandError {
    /// The program is not on `PATH` / not available to the host. P7's "runtime capability
    /// check with a clear error on mismatch, not a silent assumption."
    #[error("`{program}` was not found; callisto requires it to be available")]
    NotFound { program: String },

    /// P7's capability check, the version half. `callisto-cli`/`callisto-moon` probe
    /// `git --version` once, through the shared `callisto_graph::probe_git` helper (§G.2.4),
    /// which parses the reported string with `check_git_version` below, and fail here rather
    /// than mis-parsing an older `git`'s output.
    #[error("`{program}` reports version `{found}`, but callisto requires {required}")]
    IncompatibleVersion { program: String, found: String, required: String },

    /// The host surface cannot execute subprocesses at all — e.g. a WASM host without
    /// `exec_command`. Distinct from `NotFound` so the message can say so.
    #[error("executing `{program}` is not supported on this surface: {reason}")]
    Unsupported { program: String, reason: String },

    /// Spawn/wait/IO failure. `message` is the host's own error rendering; this crate cannot
    /// hold a `std::io::Error` because it must stay `PartialEq`-able for fixtures and must
    /// not assume a std-backed host.
    #[error("failed to run `{program}`: {message}")]
    Io { program: String, message: String },
}

/// The `git` compatibility floor both wrappers enforce, probed once per invocation. The
/// *floor* and the *parse* live here rather than being re-derived in each wrapper, since the
/// check is pure string parsing and this crate is the one both already depend on; the
/// `git --version` invocation that feeds it is `callisto_graph::probe_git` (§G.2.4), which is
/// where the `CommandRunner` call site belongs.
pub const REQUIRED_GIT: &str = ">=2.20";
pub fn check_git_version(reported: &str) -> Result<(), CommandError>;
```

> `[SPEC DECISION, not in 00-design.md: `CommandRunner`, `CommandOutput`, and `CommandError`
> live in `callisto-model`.]` §15 lists `CommandRunner` among the key traits without assigning
> it a crate, and §0.1 rule 2 calls it a core seam that must be enumerated rather than left
> implicit. It cannot live in `callisto-graph`, because `callisto-conventional` needs it for
> commit inference and does not otherwise depend on the graph; it cannot live in
> `callisto-manifests` for the same reason. `callisto-model` is the only crate that
> `callisto-graph`, `callisto-conventional`, `callisto-cli`, and `callisto-moon` all already
> depend on, and the trait itself performs no I/O — the implementations do — so §M.1.2 holds.
> Permissive licensing is a side effect, not a motivation: the seam is not a public contract
> under §0.1 rule 4, and third-party implementations are neither expected nor supported
> pre-1.0.

### M.11 Diagnostics and attribution — `diagnostic.rs`

#### M.11.1 `ConfigKey`

```rust
/// A dotted `callisto.toml` key path, e.g. `cascade.peer-escalation`. §12.5, §13 invariant 28.
///
/// A closed vocabulary of associated constants rather than free strings, because invariant 28
/// makes these keys part of the output contract: a typo'd key in an attribution line teaches
/// the user a setting that does not exist, which is worse than no attribution at all.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConfigKey(Cow<'static, str>);

impl ConfigKey {
    pub const CASCADE_MODE:                 Self = Self(Cow::Borrowed("cascade.mode"));
    pub const CASCADE_BUMP_SEVERITY:        Self = Self(Cow::Borrowed("cascade.bump-severity"));
    pub const CASCADE_PEER_ESCALATION:      Self = Self(Cow::Borrowed("cascade.peer-escalation"));
    pub const CASCADE_PRESERVE_NPM_RANGES:  Self = Self(Cow::Borrowed("cascade.preserve-npm-ranges"));
    pub const VALIDATION_ALLOW_EMPTY_CHANGESETS: Self =
        Self(Cow::Borrowed("validation.allow-empty-changesets"));
    pub const RELEASE_TRIGGER:              Self = Self(Cow::Borrowed("release-trigger"));
    pub const PRE_MAJOR_INFERENCE:          Self = Self(Cow::Borrowed("pre-major-inference"));
    pub const TAG_TEMPLATE:                 Self = Self(Cow::Borrowed("tag-template"));
    pub const FIXED_GROUP:                  Self = Self(Cow::Borrowed("fixed-group"));
    pub const LINKED_GROUP:                 Self = Self(Cow::Borrowed("linked-group"));

    pub fn as_str(&self) -> &str;
    /// Every constant above, in one list — for `callisto-cli`'s help text (§CLI.2) and for the
    /// fixture that asserts no attribution site names a key absent from this vocabulary
    /// (§M.17, §G.13).
    pub const ALL: &'static [ConfigKey];
}
```

**Attribution rendering split (§12.5 vs §18 Q5.4).** §12.5 specifies `.bumps[].governedBy` as
*"the config key responsible"* — a bare dotted string. §18 Q5.4's human-readable example shows
more: `governed by [cascade].bump-severity = "patch" (default)`. These are consistent, and the
split is deliberate: **the JSON carries the key only; the value and the `(default)` marker are
added by `callisto-cli`'s renderer** (§CLI.5.3), which has the resolved config in hand and does
not need them re-sent. This keeps the JSON field a stable one-line string per §12.5's literal
example, and keeps `callisto-graph` from having to serialize config values it merely consulted
(P6: the core computes the attribution, the wrapper renders it).

#### M.11.2 `Diagnostic`

```rust
/// A warn-by-default cross-check result, or an error, in machine-readable form.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub code: DiagnosticCode,
    pub severity: DiagnosticSeverity,
    /// Human-readable, already-formatted. The one string a wrapper may print verbatim.
    pub message: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<PackageId>,
    /// Workspace-root-relative (§M.1.3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    /// Which flag promotes this diagnostic from warning to hard failure, when one does.
    /// §6.3's `--strict` / §7.2's `--strict-graph` are independent, per-check flags that
    /// compose freely and neither of which implies the other — so the diagnostic names its
    /// own escalation flag rather than the consumer inferring it from `code`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub escalated_by: Option<StrictFlag>,
    /// The config key that could turn this check off or change its outcome, when one exists
    /// (§13 inv. 28 applied to diagnostics, not just to bumps).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub governed_by: Option<ConfigKey>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticSeverity { Warning, Error }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StrictFlag {
    /// `--strict` — this command's own warn-by-default validations (§6.3 empty-changesets,
    /// §7.5 napi drift).
    Strict,
    /// `--strict-graph` — the moon edge cross-check only (§7.2), separately named because
    /// that check is itself opt-in and must be escalatable independently.
    StrictGraph,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum DiagnosticCode {
    /// §6.3 — a changeset naming a package with zero file changes since its last-release tag.
    EmptyChangeset,
    /// §7.5 — a target in `napi.targets` that is not in the group's `members`.
    NapiTargetAddedNotInMembers,
    /// §7.5 — `members` names a platform no longer in `napi.targets`, manifest still on disk.
    NapiTargetRemovedStillOnDisk,
    /// §18 Q5.6 — napi workspace detected at v0.1/v0.2, coordination ships v0.3.
    NapiCoordinationNotYetSupported,
    /// §7.2 / decision doc change 2 — moon declared an edge the manifest walk did not find,
    /// or vice versa. Presence only; kinds are never compared (§M.8, §MO.4.4).
    GraphEdgeDisagreement,
    /// §7.3, §13 inv. 15 — a bump could not be confidently round-tripped back to a matching
    /// spec string; original left untouched.
    RangeNotRoundTrippable,
    /// §M.7.2 — a pnpm catalog spec references a workspace member that was bumped this run;
    /// callisto does not rewrite catalog entries.
    CatalogSpecNotRewritten,
    /// §M.9.3 step 4 — a tag matched a package's glob but its placeholder region was not a
    /// valid version.
    TagGlobNonVersionMatch,
    /// §18 Q4 — a `.changeset/config.json` key was dropped during `init`'s translation.
    ChangesetsConfigKeyDropped,
    /// §7.1 / §G.6.3 — `pre-major-inference` is configured for this package but is **inert**
    /// for it this run, because its current version is `0.0.z` or it has no prior release tag.
    /// §7.1 requires the tool to say so explicitly ("Inert (no remap, tool says so
    /// explicitly)"), and that requirement had no code to carry it until this variant existed.
    /// Carries `governed_by: ConfigKey::PRE_MAJOR_INFERENCE`.
    PreMajorInferenceInert,
    /// §CL.7.1 — `plan-publish` could not locate the release's section in the package's
    /// `CHANGELOG.md` (file missing, or no `## <version>` heading matching §CL.6's rule).
    /// `ReleaseEntry.changelog_section` is omitted; the plan is still valid, since a missing
    /// release note must not block a publish plan.
    ChangelogSectionNotFound,
    /// §G.4 walk — a bare [[package]] rule matches packages whose canonical
    /// manifests span two or more ecosystems; use an ecosystem-prefixed
    /// pattern such as `cargo/name` to target only one.
    BareRuleMatchesMultipleEcosystems,
    /// A napi/maturin platform triple was not recognised by triple_to_role.
    UnrecognisedPlatformTriple,
}
```

> `[SPEC DECISION, not in 00-design.md: a single `Diagnostic` type, and an optional
> `diagnostics` array on every report envelope (§M.12).]` §12.5 enumerates per-command JSON
> shapes and does not include one, but the decision doc's change 2 explicitly requires the
> moon edge disagreement to be "a field in `--format json` output so the Action/moon workflow
> can gate on it," and §6.3, §7.5, §13 inv. 15, and §18 Q5.6 each specify a warn-by-default
> check whose result a CI consumer must be able to gate on identically. One shared array with
> a coded enum is the smallest structure that serves all of them, and it is **optional** —
> `skip_serializing_if = "Vec::is_empty"` — so it length-gates rather than hard-gates in
> §12.5's contract, exactly like `.bumps[].governedBy` (§18 Q5.6 item 2). The alternative, a
> differently-named bespoke field per check per command, is the shape that makes a consumer
> re-learn the contract for every new check.

### M.12 Plan and report value types — `plan.rs`, `report.rs`

These are the **public contract** (P7, §0.1 rule 4, §16): the stable surface is this JSON on
stdout, not the Rust signatures. They live in `callisto-model`'s MIT/Apache tier precisely so
the Action, a future Nx plugin, or an independent consumer can depend on the schema types
without touching AGPL code (§16, decision doc change 4).

#### M.12.1 Envelope rules

```rust
/// Bumped atomically with the tool binary (§13 invariant 14). Every `--format json` output
/// carries it — §9.2 shows it on the plan; invariant 14 generalises it to "every
/// `--format json` output," so every report type below has the field.
pub const SCHEMA_VERSION: u32 = 1;

/// Implemented by every `--format json` payload. Exists for the §12.6 fixture harness, which
/// enumerates commands and asserts schema version + round-trip per command rather than
/// listing them by hand.
pub trait Report: Serialize + DeserializeOwned + Send + Sync + 'static {
    /// The subcommand that emits this payload — `"status"`, `"version"`, `"plan-publish"`,
    /// `"snapshot"`, `"compose-pr-body"`, `"validate"`, `"tag"`, `"init"`. Renaming a
    /// subcommand is a breaking change and must fail a fixture, not merely earn a changelog
    /// note (§12.6, §13 inv. 17).
    const COMMAND: &'static str;
    fn schema_version(&self) -> u32;
    fn diagnostics(&self) -> &[Diagnostic];
}
```

`add` and `pre` have no `Report` impl: §12.5 does not specify a JSON shape for either, and
their contract-bearing effect is on-disk (P1's byte-compatible files), not on stdout. Their
`--format json` output is an ad-hoc, unfixtured envelope owned by `callisto-cli` (§CLI.6.1,
§CLI.6.4, shape pinned in §CLI.6.12). Being unfixtured does **not** exempt them from §13
invariant 14: both envelopes carry `schemaVersion`, set from the same `SCHEMA_VERSION`
constant, because invariant 14 says "every `--format json` output" without qualification and a
consumer should never have to know which commands opted out.

Serde conventions — normative for this envelope, and uniform across every serialisable type in
`callisto-model`, not just §M.12's:

- `#[serde(rename_all = "camelCase")]` on every struct and on every `enum` whose variants are
  serialised as strings.
- **Mandatory** fields per §12.5 are plain fields with no `skip_serializing_if`; mandatory
  arrays serialize even when empty (`[]`), because §12.5 makes "absence of the key entirely"
  a hard schema failure.
- **Optional** fields per §12.5 are `Option<T>` or `Vec<T>` with
  `skip_serializing_if` + `#[serde(default)]`, so a consumer length-gates rather than
  hard-gates.
- Unknown fields are **accepted** on deserialize (no `deny_unknown_fields`): `schemaVersion`
  is the compatibility gate, and a consumer built against schema 1 must not break on a
  schema-1-compatible additive field.

> `[SPEC DECISION, not in 00-design.md: golden files in `callisto-fixtures` are 2-space
> pretty-printed with a trailing newline, and comparison is performed on *parsed* values, so
> whitespace and key order are not part of the contract.]` §6.4 pins 2-space + trailing
> newline for `pre.json` (where byte-compatibility with `@changesets/cli` demands it); nothing
> pins stdout JSON's formatting, and §12.6 asks for shape fixtures. Comparing parsed values
> keeps the fixtures from failing on a `serde_json` formatting change; pretty-printing the
> goldens keeps their diffs readable. `callisto-cli` emits the same 2-space-plus-newline shape
> on real stdout (§CLI.5.1) so that `callisto … --format json > out.json` is byte-diffable
> against a golden, even though the contract does not require it to be.

#### M.12.2 `PublishPlan` — §9.2

```rust
/// `callisto plan-publish --format json`. §9.2, §12.5.
///
/// The **ordering implied by this structure** — Rust crates, then platform npm packages, then
/// main npm packages — is the correctness-relevant fact callisto is responsible for: napi
/// main packages reference their platforms via `optionalDependencies` at exact versions, so
/// the platforms must exist on the registry first or installs 404 (§9.2, §13 inv. 8).
/// Executing the publishes in that order is the calling workflow's job (§9.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishPlan {
    pub schema_version: u32,

    /// **Array order is the intra-release topological order** (§13 invariant 7) — load-bearing,
    /// consumed positionally by §9.3's worked example, and part of the fixtured contract.
    /// The sort is scoped to the intra-release set, not the whole workspace.
    pub rust_crates: Vec<CratePublish>,

    /// napi platform packages. Present here for publishing; **never** in `releases[]` — they
    /// are dependents-in-lockstep, not independently released artefacts, and are never
    /// independently tagged (§7.5, §13 inv. 20).
    pub npm_platform_packages: Vec<NpmPublish>,

    pub npm_main_packages: Vec<NpmMainPublish>,

    /// One entry per **tag-bearing** package only (§9.2) — the napi main package and, if
    /// separately published, the crate. Computed from `Package::is_release_point` (§M.6.1),
    /// which makes invariant 20 structural.
    pub releases: Vec<ReleaseEntry>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<Diagnostic>,
}

/// A crate to publish. `name` is the **ecosystem-native** package name — the crate name, as
/// consumed by `cargo publish -p "$c"` (§9.3) — sourced from
/// `IdentityIndex::native_name(id, Ecosystem::Cargo)` (§G.4.2), never from
/// `PackageId::display_name` (different for a prefixed identity) and never from
/// `PackageId::name` (which cannot represent a Case D package whose two ecosystems declare
/// divergent native names, §M.2). Conflating any of these hands the workflow an unusable
/// argument.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CratePublish {
    pub name: String,
    pub version: Version,
    /// A `[registries.*]` key (§14): `"cratesIo"` by default. Always a plain string.
    pub publish_to: RegistryKey,
    /// Resolved registry URL, present only for a non-default registry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry: Option<String>,
}

/// An npm package to publish — a napi platform package in `npmPlatformPackages[]`, or any
/// other npm publish target. `name` is the npm package name, as consumed by
/// `pnpm -r publish --filter` (§9.3): for a `Package` it comes from
/// `IdentityIndex::native_name(id, Ecosystem::Npm)`, and for a platform manifest — which is
/// not a `Package` at all (§M.6.1's SPEC DECISION M7) — from that manifest's own registered
/// name in `IdentityIndex::platform` (§G.4.2, §G.11).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NpmPublish {
    pub name: String,
    pub version: Version,
    pub publish_to: RegistryKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NpmMainPublish {
    pub name: String,
    pub version: Version,
    pub publish_to: RegistryKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry: Option<String>,
    /// The platform package names this main package pins in `optionalDependencies` at exact
    /// versions (§7.6 step 5). Empty for a non-napi main package. Every name here appears in
    /// `npmPlatformPackages[]` of the same plan — a fixture asserts that (§CF.5).
    pub depends_on_platforms: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseEntry {
    /// Produced by the one tag-resolution function (§M.9.2, §13 inv. 25). Never assembled
    /// from `name` + `@` + `version` at this call site.
    pub tag_name: TagName,
    /// **HEAD at `plan-publish` invocation time.** Callisto never commits (§7.6 step 11), so
    /// this is necessarily after the calling workflow's own commit of `version`'s output
    /// (§9.3 runs `git commit` before `plan-publish`). A workflow that reorders those two
    /// steps gets the pre-bump commit's sha — a misuse of the contract, not an ambiguity in it.
    pub sha: CommitSha,
    /// The changelog section for this release, for a workflow that wants to attach it to a
    /// GitHub Release it creates itself (§9.5: callisto never creates one).
    ///
    /// **Optional.** Produced by reading it back out of the `CHANGELOG.md` that `version`
    /// already wrote, via `callisto_changelog::extract_section` (§CL.6, §CL.7.1) — never
    /// re-rendered, which `plan-publish` structurally cannot do anyway, since it is a
    /// separate invocation running after `version` deleted the changesets its input was built
    /// from. Absent for a package with `changelog: None` (§CL.6's opt-out — no file was ever
    /// written) and for a changelog whose section cannot be located, which also emits
    /// `DiagnosticCode::ChangelogSectionNotFound`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changelog_section: Option<String>,
}

impl Report for PublishPlan { const COMMAND: &'static str = "plan-publish"; /* … */ }
```

> `[SPEC DECISION, not in 00-design.md: plan entries' `publishTo` is a `[registries.*]` **key
> string**, with an optional sibling `registry` field for a non-default registry URL, rather
> than a serialized `PublishTarget`.]` §9.2 shows `"publishTo": "cratesIo"` and
> `"publishTo": "npm"` — plain strings — but `PublishTarget::Npm { registry: Option<String> }`
> (§5.1) has a payload, so a derived serialization would emit an object whenever a custom
> registry is configured, changing `.rustCrates[].publishTo`'s JSON type based on config. That
> would break every `jq -r` consumer §9.3 demonstrates, on exactly the workspaces most likely
> to have a custom registry. A key string plus an optional URL keeps §9.2's literal shape
> stable in all cases and matches §14, where `publish-to = ["cratesIo"]` already refers to
> registry keys.

#### M.12.3 `VersionReport` — §12.5

```rust
/// `callisto version --format json`. §12.5.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionReport {
    pub schema_version: u32,
    /// Mandatory. Possibly empty.
    pub bumps: Vec<BumpRecord>,
    /// Optional — **absent** (not empty) when `--refresh-lockfiles` was not passed (§12.5).
    /// `Option<Vec<_>>` rather than `Vec<_>` precisely so "not requested" and "requested,
    /// nothing to do" are distinguishable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lockfile_refresh_results: Option<Vec<LockfileRefreshResult>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BumpRecord {
    /// `PackageId`, serialized as its shortest unambiguous display form (§5.4). Distinct
    /// from a plan entry's ecosystem-native `name` (§M.12.2); a Case D package appears here
    /// once, under one identity, and in the plan twice, under two native names.
    pub package: PackageId,
    pub from: Version,
    pub to: Version,
    pub severity: Severity,

    /// §13 invariant 28. The config key responsible for a decision whose default could
    /// defensibly have gone the other way; absent when the bump followed no named default
    /// (§12.5). Optional, so it length-gates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub governed_by: Option<ConfigKey>,

    /// Why this bump happened, in structured form.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<BumpReason>,
}

/// The structured form of §18 Q5.4 mechanism 2's rendered attribution lines.
///
/// Exists because P6 forbids `callisto-cli` from recomputing release semantics to render
/// them: "cascade: dep @myorg/sdk 1.2.0 → 2.0.0, out of range \"^1.2\"" is a statement about
/// the cascade, computed in `callisto-graph`, and the wrapper's job is to format it, not to
/// re-derive it. §13 invariant 28's "computed in `callisto-graph`, rendered by
/// `callisto-cli`" (§18 Q5.6 item 2) is not satisfiable without this travelling in the value.
///
/// **This is the summarised, singular attribution.** A changelog needs *every* contributing
/// cause, not the dominant one, which is why `callisto-changelog` takes a richer
/// `ChangelogInput` (§CL.3) rather than a `Vec<BumpReason>` — see §CL.1's SPEC DECISION.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[non_exhaustive]
pub enum BumpReason {
    /// A changeset named this package (§7.1). Filenames, sorted, for attribution.
    Changeset { changesets: Vec<String> },
    /// Inferred from conventional commits (§7.1). `remapped` is `true` when
    /// `pre-major-inference` changed the inferred severity — the one case where callisto
    /// computes something `@changesets/cli` could not (§4 P1 scope note).
    Inference { commits: usize, remapped: bool },
    /// Unioned up by §7.1's fixed-group severity pre-step.
    FixedGroupUnion { group: GroupName },
    /// Unioned up by §7.5's linked-group joint-naming rule. Never produced by a cascade —
    /// "jointly releasing" means jointly *named*, not jointly *cascaded to* (§7.5).
    LinkedGroupUnion { group: GroupName },
    /// §7.4's cascade fired. `spec` is the original spec string that stopped covering.
    Cascade { via: PackageId, dep_kind: DepKind, spec: String, dependency_to: Version },
    /// §7.4's peer row / §13 invariant 9: out-of-range non-patch peer source escalates the
    /// dependent to major. Broken out from `Cascade` because it is the one non-inert default
    /// (§18 Q5.4) and the one most likely to need naming in output.
    PeerEscalation { via: PackageId, spec: String },
    /// §8's pre-mode counter advanced from `initialVersions`.
    PreRelease { tag: String },
    /// §7.5's new-member exemption: force-set to the group's target version on first
    /// inclusion, no divergence error.
    NewGroupMember { group: GroupName },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LockfileRefreshResult {
    /// Workspace-root-relative path to the lockfile (§M.1.3).
    pub filename: PathBuf,
    /// The command callisto ran, already argv-joined, for the log.
    pub refresh_command: String,
    pub success: bool,
    /// `null` — present, not absent — when the command could not be run at all. §12.5 lists
    /// `exitCode` as a field of this object, so it is always emitted.
    pub exit_code: Option<i32>,
}

impl Report for VersionReport { const COMMAND: &'static str = "version"; /* … */ }
```

> `[SPEC DECISION, not in 00-design.md: `BumpRecord` gains an optional `reason: BumpReason`
> beyond §12.5's `{package, from, to, severity, governedBy}`.]` §18 Q5.4 mechanism 2 shows
> human output carrying the *reason* alongside the governing key ("cascade: dep @myorg/sdk
> 1.2.0 → 2.0.0, out of range \"^1.2\""), and §18 Q5.6 item 2 places that computation in
> `callisto-graph` with rendering in `callisto-cli`; P6 forbids the wrapper from recomputing
> it. The field is optional, so it length-gates rather than hard-gates §12.5's contract, and
> a consumer that only reads `governedBy` is unaffected.

#### M.12.4 `StatusReport` — §12.5

```rust
/// `callisto status --format json`. §12.5 mandates `.hasChangesets`; §12.2 branch 3 gates on it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusReport {
    pub schema_version: u32,
    /// Mandatory (§12.5) — the field the Action's mode dispatch reads.
    pub has_changesets: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub packages: Vec<StatusEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusEntry {
    pub package: PackageId,
    /// On-disk version of record, read from a canonical manifest.
    pub current_version: Version,
    /// From `last_tag_for` (§9.1, §M.9.3). `null` when the package has no prior release tag
    /// — a routine state under P2's stateless detection, not an error (§7.1's pre-major
    /// bootstrap discussion depends on it being routine).
    pub last_tag: Option<TagName>,
    pub last_released_version: Option<Version>,
    /// Whether the package has file changes since `last_tag`. This is the input to §6.3's
    /// empty-changeset validation, surfaced here so `status` can show it before `version`
    /// enforces it.
    ///
    /// **Mandatory, and therefore computed from v0.1**, by `changed_since_last_tag` (§G.9.3),
    /// which is one `git diff --quiet` on top of the `last_tag_for` primitive §17 v0.1
    /// already commits to. §6.3's *validation* — the check, the `EmptyChangeset` diagnostic,
    /// and `allow-empty-changesets` — is the part that ships at v0.2 (§13 inv. 19, §G.14).
    /// v0.1 reports the fact; it does not enforce it.
    pub changed_since_last_tag: bool,
    /// Aggregated pending severity (§7.1), `null` when nothing names this package.
    pub pending_severity: Option<Severity>,
    pub release_trigger: ReleaseTrigger,
}

impl Report for StatusReport { const COMMAND: &'static str = "status"; /* … */ }
```

> `[SPEC DECISION, not in 00-design.md: `StatusReport.packages[]` is specified with the seven
> fields above.]` §12.5 mandates only `.hasChangesets` for `status`, but §17 v0.1 commits the
> stateless `last_tag_for` primitive as "built and used by `status` even before `plan-publish`
> ships," which requires `status` to have somewhere to report a last tag; §18 Q5.5's worked
> `init` output and the "see what a release would do" next-step line imply per-package detail.
> The array is optional (length-gated), so §12.5's contract is unchanged for a consumer that
> only reads `hasChangesets`.

#### M.12.5 `SnapshotReport`, `ComposePrBodyReport` — §12.5

```rust
/// `callisto snapshot --format json`. §8, §12.5.
///
/// Carries the computed version and the affected packages — **not** a
/// `published`/`publishedPackages` pair. `snapshot` computes and writes a transient version
/// to manifests, uncommitted and untagged; it does not publish (§8, §9).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotReport {
    pub schema_version: u32,
    /// Mandatory. The transient version, exactly `0.0.0-{tag}-{sha7}` — one value for the
    /// whole workspace, composition rule pinned in §G.11's `plan_snapshot`. §8's example
    /// `0.0.0-snapshot-<sha>` is what `--tag snapshot` produces.
    pub version: Version,
    /// Mandatory, possibly empty. `name` is the ecosystem-native package name, paired with
    /// its `ecosystem` — which is what makes this shape usable by a publish script that must
    /// route Rust and npm packages to different commands.
    pub packages: Vec<SnapshotPackage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotPackage { pub name: String, pub ecosystem: Ecosystem }

impl Report for SnapshotReport { const COMMAND: &'static str = "snapshot"; /* … */ }

/// `callisto compose-pr-body --format json`. §12.5.
///
/// **Runs before `version`** in any orchestration flow, because `version` deletes the
/// changeset files this command reads (§13 invariant 23, §12.2 branch 3). It reads the
/// changeset files directly and never reads `.callisto/plan.json` (§7.6 step 10).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposePrBodyReport {
    pub schema_version: u32,
    /// Mandatory. The composed PR body, or — when the body exceeded GitHub's size limit —
    /// the short pointer message, with `metadata.overflow` set (§12.2).
    pub body: String,
    pub metadata: PrBodyMetadata,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrBodyMetadata {
    /// Labels callisto wants applied now.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,

    /// Callisto's own last-known-applied set, round-tripped from the *previous* run's output
    /// via the existing PR body — **not** a new state file, consistent with P2 (§12.5).
    ///
    /// Semantics, confirmed against release-please's implementation: this is not a diff
    /// against arbitrary existing labels. It is "remove exactly the labels callisto itself
    /// applied last time, add exactly the labels callisto wants applied now," both drawn from
    /// a fixed, config-known set. A label present on the PR but absent from this recorded set
    /// is treated as human-applied **even if it shares a name with the current managed
    /// vocabulary**, and is left alone. A human wanting callisto to manage a label it did not
    /// apply has no supported path — P5: no implicit reconciliation, not a gap.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub managed_labels: Vec<String>,

    /// Present when the body was written to a notes branch (§12.2's overflow handling),
    /// absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overflow: Option<PrBodyOverflow>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrBodyOverflow {
    /// `<branch>--notes`. `ref` is a Rust keyword, hence the rename.
    #[serde(rename = "ref")]
    pub git_ref: String,
    pub url: String,
}

impl Report for ComposePrBodyReport { const COMMAND: &'static str = "compose-pr-body"; /* … */ }
```

#### M.12.6 `ValidateReport`, `TagReport`, `InitReport`

> `[SPEC DECISION, not in 00-design.md: §12.5 enumerates JSON shapes for `status`, `version`,
> `plan-publish`, `snapshot`, and `compose-pr-body` only, but §17 v0.2 ships `validate` and
> `callisto tag`, and §18 Q5.5 states that `init` supports `--format json` and that its output
> is the moon-side payload for `initialize_extension` (§10/§11). §13 invariant 14 requires a
> schema version on *every* `--format json` output, so these three cannot be left shapeless.]`
> The shapes below are deliberately minimal — each is the smallest thing that lets a wrapper
> gate on the command's outcome — and each is a candidate for expansion when its milestone
> lands.

```rust
/// `callisto validate --format json` (v0.2, §17, §18 Q3). Used by the Action's
/// `validate-on-pr` gate (§12.2 branch 1), which fails the check on errors.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidateReport {
    pub schema_version: u32,
    /// `false` iff any diagnostic has `severity == Error` (after `--strict`/`--strict-graph`
    /// escalation has been applied). The single field a gate needs to branch on.
    pub ok: bool,
    /// Mandatory here, unlike every other report — `validate`'s entire payload *is* its
    /// diagnostics, so an absent key would leave the command with nothing to say.
    pub diagnostics: Vec<Diagnostic>,
}

/// `callisto tag --format json` (v0.2, §17). Creates **local tags only, never pushes**
/// (§9.1); pushing stays the calling workflow's job (§13 invariant 16's push-last rule).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagReport {
    pub schema_version: u32,
    pub tags: Vec<CreatedTag>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedTag {
    pub tag_name: TagName,
    pub sha: CommitSha,
    /// `true` when the tag already existed at this sha and was left alone — P3's idempotence
    /// made observable rather than assumed. An existing tag at a *different* sha is an error
    /// diagnostic, not a silent overwrite.
    pub already_existed: bool,
}

/// `callisto init --format json` (v0.1, §18 Q5.5). Also the moon-side payload for
/// `initialize_extension` (§10, §11, §MO.2.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitReport {
    pub schema_version: u32,
    /// `"moon"` or `"ignore-walk"` — §18 Q5.5's "project locator" line.
    pub locator: String,
    pub packages: Vec<InitPackage>,
    /// Files written, workspace-root-relative.
    pub written: Vec<PathBuf>,
    /// The proposed `callisto.toml` content when `init` is running as a reconcile/migration
    /// flow and is showing a diff for review rather than writing (§5.3, §18 Q4, §13 inv. 21).
    /// `null` when `init` wrote directly or had nothing to propose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposed_config: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitPackage {
    pub package: PackageId,
    pub manifests: Vec<ManifestDecl>,
    pub publish_to: Vec<PublishTarget>,
    pub release_trigger: ReleaseTrigger,
}

impl Report for ValidateReport { const COMMAND: &'static str = "validate"; /* … */ }
impl Report for TagReport      { const COMMAND: &'static str = "tag";      /* … */ }
impl Report for InitReport     { const COMMAND: &'static str = "init";     /* … */ }
```

#### M.12.7 `MatrixReport` — §19, §G.11 `matrix`, §CLI.6.13

`callisto matrix`'s report is keyed by package name rather than shaped as a per-`Package` list
like every other report above, because two packages can independently populate the *same* two
maps without either being "the" subject of the report — there is no natural single-array shape
that avoids repeating each package's identity once per contributing field.

```rust
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MatrixReport {
    pub schema_version: u32,
    /// Keyed by `PackageId::name()`. `BTreeMap` gives lexicographic key order for free — no
    /// separate sort step (AC-009). A package that declares neither `napi.targets` nor
    /// `[tool.maturin].targets` contributes no entry; the map is `{}`, not omitted, for a
    /// workspace with no platform packages at all (§13 invariant 14's schema-version-on-every-
    /// output rule still needs a report to attach to).
    pub platform_targets: BTreeMap<String, PlatformTargetGroup>,
    /// Same keying rule, independent of `platform_targets` — a package may populate either map,
    /// both, or neither.
    pub runtime_versions: BTreeMap<String, Vec<RuntimeVersionEntry>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PlatformTargetGroup {
    pub kind: PlatformTargetKind,
    /// The raw manifest field name (`"napi.targets"` or `"[tool.maturin].targets"`) — used for
    /// audit output only, never a filesystem path.
    pub source: String,
    /// Sorted ascending by `triple` before serialization (AC-009).
    pub targets: Vec<PlatformTarget>,
}

/// Deliberately two variants. No `DotnetAot` — out of scope for this spec (§G.11's SPEC
/// DECISION on escalation makes the same "not yet" call for the diagnostics that would
/// accompany it). Deserializing an unrecognised variant string is a hard error, not a silent
/// fallback to one of the two real ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PlatformTargetKind { Napi, Maturin }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PlatformTarget {
    pub triple: String,
    pub platform: String,
    pub arch: String,
    /// `null` for every non-Linux platform family; a string for every Linux triple (AC-014).
    /// No `skip_serializing_if` — the key is always present, `null` or not, so a consumer never
    /// has to distinguish "no abi" from "field omitted."
    pub abi: Option<String>,
    pub host_runner: String,
    pub use_cross: bool,
    /// Always `"native-" + triple` (AC-013).
    pub artifact_name: String,
    /// Workspace-root-relative.
    pub package_dir: String,
    pub package_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeVersionEntry {
    pub ecosystem: RuntimeEcosystem,
    /// `"engines.node"` or `"requires-python"`.
    pub field: String,
    /// The raw, unvalidated manifest string — `matrix` reports what a manifest declares, it
    /// does not parse or validate the range grammar.
    pub range: String,
}

/// Deliberately two variants, same reasoning as `PlatformTargetKind`. No `Dotnet`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeEcosystem { Npm, Python }

impl Report for MatrixReport { const COMMAND: &'static str = "matrix"; /* … */ }
```

#### M.12.8 What is deliberately *not* a model type

§12.4's `published-packages` output — `[{name, version, ecosystem, publishedTo}]` — is
**not** declared here. §12.4 is explicit that it "reflects what the *workflow* reported back
via the `publish:` command's own exit status and output, not something callisto observed
directly," and §9 makes publishing always the calling workflow's step. A model type for it
would imply callisto produces it, which is the orchestrator posture §9.5 removed. The Action
composes it in bash from the plan plus its own publish step's results.

### M.13 Errors — `error.rs`

#### M.13.1 `ModelError` (this crate's own)

```rust
/// Errors from this crate's own validating constructors. Small on purpose — this crate
/// validates shapes, it does not perform operations.
#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum ModelError {
    #[error("path `{path}` is absolute; callisto-model paths are workspace-root-relative")]
    AbsolutePath { path: String },

    #[error("path is not valid UTF-8; callisto serializes paths into its JSON contract")]
    NonUtf8Path,

    #[error("`{raw}` is not a valid 40-character hexadecimal commit sha")]
    InvalidCommitSha { raw: String },

    #[error("manifest role {role:?} is not valid for format {format:?}")]
    InvalidRoleForFormat { role: ManifestRole, format: ManifestFormat },

    #[error("package `{package}` has no canonical manifest; at least one is required")]
    NoCanonicalManifest { package: PackageId },

    #[error("package `{package}` has canonical manifests in disagreeing version grammars \
             ({grammars:?}); its version of record has no single grammar")]
    MixedVersionGrammars { package: PackageId, grammars: Vec<VersionGrammar> },
}
```

#### M.13.2 `ManifestError`

> `[SPEC DECISION, not in 00-design.md: `ManifestError` is declared in `callisto-model`, not
> in `callisto-manifests`.]` §15 assigns `LocateError` and `GraphError` to `callisto-graph`
> explicitly but says nothing about `ManifestError`. It goes here because `callisto-manifests`
> is feature-flagged per ecosystem (§15, §CM.8), so an error enum declared there would vary in
> shape with the enabled feature set — making a `match` in `callisto-graph` compile or not
> depending on features, which is a build-configuration failure mode rather than a code one.
> `callisto-graph` consumes manifests through `ManifestWalkResolver` and must be able to name
> every variant unconditionally. This is easy to re-litigate: moving it costs one `pub use`.

```rust
/// Errors from reading or writing one manifest file. The error half of the `Manifest` trait
/// (§15, §CM.1), whose implementations live in `callisto-manifests`.
///
/// Every variant carries the workspace-relative `path`, because "which file" is the first
/// question a user asks and the manifest handle is gone by the time the error is rendered.
#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum ManifestError {
    #[error("failed to read `{path}`: {message}")]
    Read { path: PathBuf, message: String },

    #[error("failed to write `{path}`: {message}")]
    Write { path: PathBuf, message: String },

    #[error("`{path}` is not valid {format:?}: {message}")]
    Parse { path: PathBuf, format: ManifestFormat, message: String },

    #[error("`{path}` has no `{field}` field")]
    MissingField { path: PathBuf, field: &'static str },

    #[error("`{path}` declares `{raw}` as its version, which is invalid: {source}")]
    InvalidVersion { path: PathBuf, raw: String, #[source] source: VersionParseError },

    /// Cargo's `version.workspace = true` / `foo.workspace = true` (§18 Q2). The write target
    /// is the workspace root, not this file. `WorkspaceCargoResolver` (§17 v0.1, §CM.4.4)
    /// resolves these; this variant is what a *direct* write attempt on an inherited value
    /// produces, so a missed resolution fails loudly instead of writing a member-local
    /// override that silently shadows the workspace value.
    #[error("`{path}` inherits `{key}` from the workspace root; write the root manifest instead")]
    WorkspaceInherited { path: PathBuf, key: String },

    /// §7.8's "hard error, not silent skip" posture: `setup.cfg`-only Python projects,
    /// imperative `build.gradle`/`build.sbt`, and lockfiles are read-only or unwritable, and
    /// callisto says so and directs the user to the supported convention.
    #[error("`{path}` ({format:?}) is not a supported write target: {reason}")]
    ReadOnlyFormat { path: PathBuf, format: ManifestFormat, reason: &'static str },

    #[error("`{path}` has no `{kind:?}` dependency named `{name}`")]
    DependencyNotFound { path: PathBuf, name: String, kind: DepKind },

    /// e.g. `update_optional_dependencies` on a `Cargo.toml` (§CM.6).
    #[error("operation `{operation}` is not supported for `{path}` ({format:?})")]
    UnsupportedOperation { path: PathBuf, format: ManifestFormat, operation: &'static str },

    /// The format-preserving editor could not reproduce the file's original formatting.
    /// Distinct from `Write` so §12.6's round-trip-fidelity fixtures fail with a specific
    /// message rather than a generic IO one.
    #[error("format-preserving write of `{path}` would not round-trip: {message}")]
    FormattingNotPreserved { path: PathBuf, message: String },
}
```

Note what is **not** here: a range-not-round-trippable variant. §13 invariant 15 makes that a
warn-and-leave-untouched outcome, not a failure — it is
`DiagnosticCode::RangeNotRoundTrippable` (§M.11.2). Modelling it as an error would make the
default behaviour "abort the version pass," which is the opposite of what the invariant says.

#### M.13.3 `LocateError` and `GraphError` — declared in `callisto-graph`

§15: *"`LocateError` and `GraphError` are ordinary per-crate error enums (`callisto-graph`),
not given full signatures here."* This spec **keeps that placement** — they are not
`callisto-model` exports. `LocateError`'s full definition is pinned here anyway, and `GraphError`'s
`Locate` through `Command` variant range (below), mostly transparent wrappers, are pinned in
their real declaration order, because the values they accompany (`ProjectRoot`, `DeclaredEdge`,
`PackageId`, `Version`) are model types and the two crates' specs must agree on the vocabulary.
`#[diagnostic(...)]` attributes and the `#[allow(clippy::result_large_err)]`/enum-level
`#[diagnostic]` derive real source also carries are omitted here for readability. All 21 of
`GraphError`'s coded (non-transparent) variants are shown below: 19 grouped together above, plus
`ParseChangeset` and `Command`, which sit among the otherwise-transparent wrapper range in real
source's own declaration order (neither is itself transparent — see their own doc comments
below). The wrapper variants themselves match real source exactly, field for field. All 21
coded variants match real source exactly too — fields and `#[error(...)]` message text alike
(mod `{name}`-style named interpolation standing in for an equivalent explicit method call
real source writes instead, e.g. `{v}` for `.render()`, `{id}` for `.display_name()` — provably
identical output, not a wording difference).

```rust
// crates/callisto-graph/src/locate.rs
#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum LocateError {
    #[error("no workspace root found (no Cargo.toml [workspace], package.json \"workspaces\", \
             pnpm-workspace.yaml, or .moon/ above the current directory)")]
    WorkspaceRootNotFound,

    #[error("failed to walk `{path}`: {message}")]
    Walk { path: PathBuf, message: String },

    /// `MoonProjectLocator` could not obtain moon's project graph.
    #[error("moon is unavailable: {message}")]
    MoonUnavailable { message: String },

    #[error("could not parse `moon project-graph --json` output: {message}")]
    MoonOutputParse { message: String },

    /// P7's runtime capability check. `callisto-moon` pins a specific moon compatibility
    /// range (§15, §MO.0) and fails here rather than mis-reading a shape that moved.
    #[error("moon {found} is outside callisto's supported range ({supported})")]
    IncompatibleMoonVersion { found: String, supported: String },

    /// A located path escapes the workspace root — invisible under moon's preopened-directory
    /// sandbox (§0.1 rule 2), so it is refused at the boundary rather than failing later.
    #[error("`{path}` is outside the workspace root and cannot be addressed")]
    OutsideWorkspaceRoot { path: String },

    #[error(transparent)]
    Manifest(#[from] ManifestError),

    #[error(transparent)]
    Command(#[from] CommandError),

    #[error(transparent)]
    Model(#[from] ModelError),
}

// crates/callisto-graph/src/error.rs
#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum GraphError {
    /// `DependencyResolver::toposort` (§15). The cycle is reported in traversal order so the
    /// message names a walkable path, not a set.
    #[error("dependency cycle detected: {}", .cycle.iter().map(|i| i.display_name())
                                          .collect::<Vec<_>>().join(" -> "))]
    Cycle { cycle: Vec<PackageId> },

    #[error("package `{id}` was not found in the workspace")]
    UnknownPackage { id: PackageId },

    /// §14: "two sets claiming the same package is a hard config error"; also two discovered
    /// projects resolving to one identity.
    #[error("package `{id}` is defined at multiple paths: {}", .paths.iter()
                .map(|p| p.display().to_string()).collect::<Vec<_>>().join(", "))]
    DuplicatePackage { id: PackageId, paths: Vec<PathBuf> },

    /// §5.4: "error listing candidates."
    #[error("name `{name}` is ambiguous in this workspace; candidates: {}", .candidates.iter()
                .map(|c| c.display_name()).collect::<Vec<_>>().join(", "))]
    AmbiguousName { name: String, candidates: Vec<PackageId> },

    /// §5.4(b): a Case D root whose two canonical manifests resolved to different identities.
    /// The single-`Package`, aggregate-by-max model depends on this never happening silently
    /// (real source's message itself doesn't spell out the "dual-published" framing — that's
    /// this comment's context, not the `#[error(...)]` text).
    #[error("package at `{path}` declares conflicting identities: {}",
             .ids.iter().map(|i| i.display_name()).collect::<Vec<_>>().join(", "))]
    SplitIdentity { path: PathBuf, ids: Vec<PackageId> },

    /// A dependency edge whose two endpoints parsed under different `VersionGrammar`s —
    /// the shape §G.7.4's cascade produces, where `from`/`to` are the edge's own endpoints.
    #[error("version dependency edge from `{from}` to `{to}` involves incompatible grammars: \
             {source}")]
    GrammarMismatch { from: PackageId, to: PackageId, #[source] source: GrammarMismatch },

    /// A group-internal version comparison with mixed grammars — §G.6.7's linked-group joint
    /// version union and §G.8.2/§G.8.3's fixed-group alignment/aligned-base fallback all
    /// compare versions across a *group's members*, which has no natural edge to hang a
    /// `from`/`to` pair on, unlike the variant above. Kept distinct rather than reusing
    /// `GrammarMismatch` with placeholder endpoints, since a fabricated edge would misdescribe
    /// what actually happened. The message shows each member's grammar only (via
    /// `Version::grammar()`), not its version value — there is no separate
    /// `source: GrammarMismatch` field.
    #[error("group `{group}` members use incompatible versioning grammars: {}", .members.iter()
                .map(|(id, v)| format!("{}={:?}", id.display_name(), v.grammar()))
                .collect::<Vec<_>>().join(", "))]
    GroupGrammarMismatch { group: GroupName, members: Vec<(PackageId, Version)> },

    /// §7.5's pre-mutation fixed-group alignment check (§7.6 step 2, §13 inv. 13). Compares
    /// **on-disk** versions, in pre-mode exactly as in normal mode — `initialVersions` is a
    /// bump-computation input only, never an alignment-check input (§8). Members with no
    /// prior release tag are exempt and never reach this variant (§7.5, §13 inv. 22).
    #[error("fixed group `{group}` members have divergent on-disk versions: {}", .members.iter()
                .map(|(id, v)| format!("{id}={v}")).collect::<Vec<_>>().join(", "))]
    FixedGroupDivergent { group: GroupName, members: Vec<(PackageId, Version)> },

    /// `GroupTable::resolve` (§G.5.5): a `[[fixed-group]]`/`[[linked-group]]` config `member`
    /// name resolves to neither a real package identity (via `IdentityIndex::resolve_human`)
    /// nor a platform manifest name (via `IdentityIndex.platform`) — a config-declared member
    /// that does not exist in the workspace at all under either lookup, so it fails loudly
    /// (P5) rather than being silently skipped. `member` carries the raw, unresolved config
    /// string, not a `PackageId` (there is no identity to name — resolution is exactly what
    /// failed).
    #[error("group `{group}` lists member `{member}`, which was not found in the workspace")]
    MissingGroupMember { group: GroupName, member: String },

    /// §14's parse-time group validation (§18's group-priority follow-on): a package must
    /// belong to at most one fixed group and at most one linked group, and the two member
    /// sets must be disjoint. Rejected at parse time rather than arbitrated at runtime.
    #[error("package `{package}` is listed in multiple conflicting groups: {}", .groups.iter()
                .map(|g| g.as_str()).collect::<Vec<_>>().join(", "))]
    ConflictingGroupMembership { package: PackageId, groups: Vec<GroupName> },

    /// §7.6 step 1's rerun-safety check (§13 inv. 12): the on-disk version moved between
    /// aggregation and mutation. Real source's message does not itself suggest a fix — the
    /// "re-run `callisto version`" advice used to live in this `#[error(...)]` text but real
    /// source never carried it; that's this comment's context, not the message.
    #[error("on-disk versions changed since plan was generated for `{package}`: expected \
             {expected}, found {found}")]
    OnDiskVersionDrift { package: PackageId, expected: Version, found: Version },

    /// §7.4 runs to fixpoint; this is the safety bound that turns a would-be hang into a
    /// reportable bug (P5). See §G.7.6 for why the bound is derived rather than tuned.
    #[error("cascade failed to converge after {iterations} iterations")]
    CascadeNotConverged { iterations: usize },

    /// `apply_version_plan`'s rerun-safety check for a single manifest write (§7.6 step 1's
    /// package-level sibling to `OnDiskVersionDrift` above): the manifest at `path` is at
    /// neither the plan's `from` nor `to` version when the write is about to happen. Real
    /// call sites (`apply.rs`) take a separate early branch whenever `found` already equals
    /// the target version — this variant is only ever constructed once that branch is ruled
    /// out, so `found` here is guaranteed to differ from *both* `expected_from` and
    /// `expected_to`; there is no "already applied" case reachable through this error.
    #[error("cannot apply version plan: manifest `{path}` is at version {found}, expected \
             {expected_from} (pre-apply) or {expected_to} (already applied — safe to retry)")]
    UnexpectedManifestVersion {
        path: PathBuf,
        expected_from: Version,
        expected_to: Version,
        found: Version,
    },

    /// §G.10.2 step 3's *intended* guard: two packages' bumps both resolving to
    /// `[workspace.package].version` at the same Cargo root
    /// (`VersionWriteTarget::CargoWorkspacePackage`, §G.10.1) but wanting different versions
    /// should be refused before any write happens, since members sharing one inherited
    /// version cannot diverge and letting the last write win would pick a winner by iteration
    /// order (§18 Q2, P5). As of this writing, however, this variant is never constructed —
    /// grep confirms no production call site builds it, and `apply_version_plan`
    /// (`apply.rs`) applies each `CargoWorkspacePackage` write independently with no
    /// cross-bump conflict check, so the last-write-wins outcome this variant exists to
    /// prevent is not actually prevented today. `details` is a caller-assembled description
    /// (this variant does not build its own message from structured fields the way its
    /// sibling coded variants do).
    #[error("workspace root `{root_manifest}` has conflicting version updates: {details}")]
    WorkspaceVersionConflict { root_manifest: PathBuf, details: String },

    /// `.changeset/pre.json` failed to parse as valid pre-mode state (§8) — the file exists
    /// and was read, but `callisto_format::parse_pre_json` rejected its contents.
    #[error("failed to parse .changeset/pre.json: {0}")]
    PreJson(callisto_format::PreJsonError),

    /// `.changeset/pre.json` exists but couldn't be read at all (permissions, I/O failure) —
    /// distinct from `PreJson` above, which is a read that succeeded but a parse that didn't.
    #[error("failed to read .changeset/pre.json: {message}")]
    PreJsonRead { message: String },

    /// §G.11's `matrix` (AC-017): a package declares platform targets via both
    /// `napi.targets` (`package.json`) and `[tool.maturin].targets` (`pyproject.toml`) in the
    /// same directory. Exactly one source is allowed per package — accepting both would leave
    /// no principled way to decide which list is authoritative, so it is refused rather than
    /// silently preferring one.
    #[error("package `{package}` declares platform targets via both `{napi_source}` and \
             `{maturin_source}`; only one source is allowed")]
    ConflictingPlatformTargetSources {
        package: PackageId,
        napi_source: &'static str,
        maturin_source: &'static str,
    },

    /// A resolved `[[package]]`/`[[package-set]]` `publish-to` override names a target whose
    /// `.ecosystem()` (e.g. `nuget`) is not one of `package`'s own detected ecosystems (e.g. a
    /// Cargo-only crate) — caught in `walk.rs`'s identity-building loop, the only place both
    /// the resolved override and the package's real, detected ecosystems are known at once.
    /// Rejected rather than silently accepted, since a silently-accepted mismatch makes the
    /// package vanish from every real publish downstream with zero diagnostic.
    #[error(
        "package `{package}` configures publish-to target `{target}` (ecosystem `{}`), but \
         its detected ecosystem is `{}`",
        .target_ecosystem.prefix(),
        .package_ecosystems.iter().map(|e| e.prefix()).collect::<Vec<_>>().join(", ")
    )]
    PublishTargetEcosystemMismatch {
        package: PackageId,
        target: String,
        target_ecosystem: Ecosystem,
        package_ecosystems: Vec<Ecosystem>,
    },

    /// `package.json`'s `publishConfig.registry` is manifest-controlled data (a PR author can
    /// set it), never trusted verbatim as a publish destination: `url` must use `https` and
    /// exactly match a `url` configured on an `npm`-kind `[registries]` entry in
    /// `callisto.toml`. An npm package setting a registry that isn't operator-approved this
    /// way hits this variant rather than being silently published to an untrusted host.
    #[error("package `{package}` sets `publishConfig.registry` to `{url}`, which is not an \
             operator-approved npm registry")]
    UntrustedNpmRegistry { package: PackageId, url: String },

    #[error(transparent)]
    Locate(#[from] LocateError),
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    /// Added by §G.12.
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Format(#[from] callisto_format::ParseError),
    /// A changeset file's own parse failure, distinguished from the bare transparent `Format`
    /// wrapper above by carrying which changeset `path` failed — `Format` alone loses that
    /// context, and a parse failure deep in `load_changesets` (§G.6.1) needs to name the file.
    #[error("parsing changeset {}: {source}", .path.display())]
    ParseChangeset { path: PathBuf, source: callisto_format::ParseError },
    #[error(transparent)]
    Bump(#[from] callisto_format::BumpError),
    /// Added by §CL.8's placement: `apply_version_plan` step 7 delegates to
    /// `callisto-changelog`, whose failures must propagate through the one error type
    /// `callisto-cli` matches on.
    #[error(transparent)]
    Changelog(#[from] callisto_changelog::ChangelogError),
    /// Added by §C.8's placement, reachable only when `callisto-graph`'s `inference` feature
    /// is enabled (§G.6.4).
    #[cfg(feature = "inference")]
    #[error(transparent)]
    Conventional(#[from] callisto_conventional::ConventionalError),
    #[error(transparent)]
    TagTemplate(#[from] TagTemplateError),
    #[error(transparent)]
    VersionParse(#[from] VersionParseError),
    #[error(transparent)]
    Model(#[from] ModelError),
    /// Transparent from `callisto-vcs` (§V.2) — a failing `git` operation reached through
    /// `Workspace::git_access()`/`TagIndex::build`/`plan_publish`'s `head_sha` resolution and
    /// the other `GitAccess` call sites across this crate.
    #[error(transparent)]
    Vcs(#[from] callisto_vcs::VcsError),
    #[error("command error: {0}")]
    Command(#[from] CommandError),
}
```

### M.14 Cross-crate consumption

Every type below is `Send + Sync + 'static` (§M.1.5); the column notes where that bound is
actually load-bearing.

| Type | `format` | `graph` | `manifests` | `conventional` | `changelog` | `cli` | `moon` | `fixtures` | Bound is load-bearing because |
|---|:--:|:--:|:--:|:--:|:--:|:--:|:--:|:--:|---|
| `Ecosystem` | | ✓ | ✓ | | ✓ | ✓ | ✓ | ✓ | `Manifest::ecosystem()` on a `Send + Sync` trait |
| `PackageId` | | ✓ | | ✓ | ✓ | ✓ | ✓ | ✓ | `ProjectRoot`, `DeclaredEdge`, `DepEdge` fields |
| `GroupName` | | ✓ | | | ✓ | ✓ | | ✓ | `BumpReason` field |
| `GroupKind` | | ✓ | | | ✓ | ✓ | | ✓ | `GroupDef`/`ChangeSource::GroupUnion` field |
| `RegistryKey` | | ✓ | ✓ | | | ✓ | ✓ | ✓ | plan entries |
| `CommitSha` | | ✓ | | ✓ | ✓ | ✓ | ✓ | ✓ | `ReleaseEntry`, `CreatedTag` |
| `Version` | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | `Manifest::current_version/write_version` |
| `VersionReq` | | ✓ | ✓ | | | | | ✓ | `DepSpec::Range` inside `DependencyEntry` |
| `VersionGrammar` | ✓ | ✓ | ✓ | ✓ | | ✓ | | ✓ | — |
| `Severity` | ✓ | ✓ | | ✓ | ✓ | ✓ | ✓ | ✓ | `BumpRecord` |
| `Package` | | ✓ | | | ✓ | ✓ | | ✓ | `DependencyResolver::packages() -> impl Iterator<Item = &Package>` requires `Package: Sync` |
| `ManifestDecl` | | ✓ | ✓ | | | ✓ | ✓ | ✓ | constructed into `Manifest` handles |
| `ManifestRole` | | ✓ | ✓ | | | ✓ | | ✓ | `Manifest::role()` |
| `ManifestFormat` | | ✓ | ✓ | | | ✓ | ✓ | ✓ | — |
| `ReleaseTrigger` | | ✓ | | ✓ | | ✓ | ✓ | ✓ | — |
| `PublishTarget` | | ✓ | ✓ | | | ✓ | ✓ | ✓ | — |
| `DepKind` | | ✓ | ✓ | | ✓ | ✓ | ✓ | ✓ | `Manifest::update_dependency_spec` |
| `DepSpec` | | ✓ | ✓ | | | ✓ | | ✓ | `Manifest::iter_dependencies` yields it |
| `WorkspaceKind` | | ✓ | ✓ | | | | | ✓ | inside `DepSpec` |
| `Coverage` | | ✓ | | | | ✓ | | ✓ | — |
| `DependencyEntry` | | ✓ | ✓ | | | | | ✓ | `Manifest::iter_dependencies` item type |
| `DepEdge` | | ✓ | | | ✓ | ✓ | | ✓ | `DependencyResolver::dependencies_of() -> impl Iterator<Item = &DepEdge>` requires `DepEdge: Sync` |
| `ProjectRoot` | | ✓ | | | | ✓ | ✓ | ✓ | `ProjectLocator::projects() -> Result<Vec<ProjectRoot>, _>` |
| `DeclaredEdge` / `DeclaredEdgeKind` | | ✓ | | | | ✓ | ✓ | ✓ | `ProjectLocator::declared_edges()` |
| `TagTemplate` / `TagName` / `LastTag` | | ✓ | | | ✓ | ✓ | ✓ | ✓ | §13 inv. 25's single resolution path |
| `CommandRunner` | | ✓ | | ✓ | | ✓ | ✓ | ✓ | declared `Send + Sync` in §15 |
| `CommandOutput` / `CommandError` | | ✓ | | ✓ | | ✓ | ✓ | ✓ | `CommandRunner::run`'s `Result` |
| `Diagnostic` and friends | ✓ | ✓ | | ✓ | ✓ | ✓ | ✓ | ✓ | — |
| `ConfigKey` | | ✓ | | | | ✓ | | ✓ | §13 inv. 28 |
| plan/report types (§M.12) | | ✓ | | | ✓ | ✓ | ✓ | ✓ | the §12.5 contract; `Report: Send + Sync + 'static` |
| `ManifestError` | | ✓ | ✓ | | | ✓ | ✓ | ✓ | `Manifest`'s `Result` type |
| `ModelError` | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | — |

Notes on three rows worth calling out:

- **`callisto-format` consumes very little.** `Version`, `Severity`, `VersionGrammar`,
  `Diagnostic`, and `ModelError` — that is the whole surface. §M.4.5's crate edge is real but
  narrow, which is what keeps §F.2's "the primitive worth spreading" claim honest.
- **`callisto-fixtures` uses everything.** It holds the in-memory `DependencyResolver` impl —
  the named second implementor that justifies the trait existing at all (decision doc change
  3) — which constructs graphs from literal `Package`/`DepEdge` values with no filesystem.
  That is only possible because both types are plain owned data with no handles in them.
- **`callisto-cli` is on many rows but constructs almost nothing.** It parses argv, renders,
  and does process I/O (§15). §13 invariant 27 CI-enforces that it contains no
  graph-construction or cascade code; its use of these types is read-and-render.

### M.15 Deliberately not owned by this crate

| Concept | Owner | Why not here |
|---|---|---|
| `bump_version`, the `Versioning` trait | `callisto-format` | Decision doc: `bump_version` stays in the MIT crate as the canonical Rust implementation of changesets semantics with a pure byte/behaviour-compat fixture suite (§6.2, §7.1). This crate owns version *values*; that crate owns applying a severity to one. |
| `Changeset`, `PreState` (`pre.json`) | `callisto-format` | §6, §6.4 — the byte-compatible on-disk format, with its own fixture corpus. |
| `Manifest` trait, `ManifestWalkResolver`, `WorkspaceCargoResolver` | `callisto-manifests` / `callisto-graph` | Runtime file handles and per-ecosystem read/write, feature-flagged (§15, §17 v0.1). `ManifestWalkResolver` is `callisto-graph`'s (§G.4), since it constructs `Package`s from manifests rather than editing one. |
| `ProjectLocator`, `DependencyResolver`, `IgnoreWalkLocator`, cascade, groups, `last_tag_for`'s git call | `callisto-graph` | §15. `DependencyResolver` keeps `-> impl Iterator` and static dispatch; no boxing, no dyn-compatibility, no pre-1.0 stability promise (decision doc change 3). |
| `MoonProjectLocator`, the `DependencyScope → DeclaredEdgeKind` mapping | `callisto-moon` | §15; pinned to a moon compatibility range and expected to break pre-1.0. |
| `callisto.toml` parsing and the resolved-config type (`[[package-set]]`, `[[package]]`, `[[fixed-group]]`, `[[linked-group]]`, `[cascade]`, `[validation]`, `[registries.*]`, `pre-major-inference`) | `callisto-graph::config` (§G.5) | §14. This crate declares `Package` as the *resolved* product, plus `GroupName`/`ConfigKey` so decisions can be attributed without depending on the parser. Group membership is a relation over packages, not a field on one (§M.6.1). |
| `.callisto/plan.json` | `callisto-cli` | §7.6 step 10 — `.gitignore`d, never load-bearing (P2, §13 inv. 4), and explicitly **not** the versioned contract (decision doc rule 4). `PublishPlan` is the stdout shape; whether a copy is also dropped on disk is an I/O concern. |
| `published-packages` (§12.4) | `orin-dx/callisto-action` | §M.12.8. |
| `refs/callisto/pre-cursor/<PackageId>` (§8) | `callisto-conventional` (§C.6) | A git ref namespace, not a value type. Its *name* is derived from `PackageId::display_name`, so this crate supplies the string; the ref itself is I/O. |

### M.16 Index of `[SPEC DECISION]` flags

| # | Section | Decision |
|---|---|---|
| 1 | §M.1.3 | All model paths are workspace-root-relative and UTF-8; validating constructors reject otherwise. |
| 2 | §M.4.1 | `Version` is a grammar-tagged struct, not a newtype over `semver::Version`. |
| 3 | §M.4.2 | `Version`'s `Deserialize` parses under `SemVer`; a non-SemVer ecosystem requires a grammar discriminator plus a `schemaVersion` bump. |
| 4 | §M.4.2 | `Ord` is deliberately not implemented for `Version`; cross-grammar comparison is `Err`/`None`. |
| 5 | §M.4.5 | `Severity`/`Version`/`VersionGrammar`/`VersionReq` are canonical in `callisto-model`; the crate dependency edge is `callisto-format → callisto-model`, and `callisto-model` depends on no `callisto-*` crate. |
| 6 | §M.6.1 | `Package` keeps exactly §5.1's six fields; `pre-major-inference` and group membership live with resolved config. |
| 7 | §M.6.1 | napi platform packages are `ManifestRole::Platform` manifests of the main `Package` (§5.2 authoritative), not separate `Package` values; `[[fixed-group]] members` is the config naming surface. Makes §13 inv. 20 structural. |
| 8 | §M.7.2 | `DepSpec::Catalog` is `Coverage::Unknown` and never rewritten, routed through §13 inv. 15's warn-and-leave-alone path. |
| 9 | §M.7.3 | `DepEdge` gains `from_manifest: PathBuf`; graph construction emits one edge per (declaring manifest, dependency entry). |
| 10 | §M.8 | `ProjectLocator` emits one `ProjectRoot` per (root path, ecosystem); Case D collapse stays in graph construction, keeping §15's declared field shape. |
| 11 | §M.9.1 | A template with no literal anchor (`"{version}"`) is rejected at config-load (`NoLiteralAnchor`). |
| 12 | §M.9.4 | `last_tag_for` is split: pure glob/extract/select in `callisto-model`, the `git tag --list` call in `callisto-graph`. |
| 13 | §M.10 | `CommandRunner`/`CommandOutput`/`CommandError` (and the shared `git` version floor, `REQUIRED_GIT`/`check_git_version`) live in `callisto-model`, and the trait is kept dyn-compatible. |
| 14 | §M.11.2 | One `Diagnostic` type plus an optional `diagnostics` array on every report envelope. |
| 15 | §M.12.1 | Golden files are 2-space pretty-printed with trailing newline; comparison is on parsed values. |
| 16 | §M.12.2 | Plan `publishTo` is a registry-key string with an optional sibling `registry` URL. |
| 17 | §M.12.3 | `BumpRecord` gains an optional structured `reason: BumpReason`. |
| 18 | §M.12.4 | `StatusReport.packages[]` shape specified (optional, length-gated). |
| 19 | §M.12.6 | Minimal shapes specified for `validate`, `tag`, and `init` JSON output. |
| 20 | §M.13.2 | `ManifestError` is declared in `callisto-model`, not `callisto-manifests` (feature-flag reason). |
| 21 | §M.7.3 | Workspace inheritance is signalled by `DependencyEntry::inherited` (and carried onto `DepEdge::inherited`), not by a `DepSpec` variant or method. |
| 22 | §M.2 | `PackageId::name()` is the identity's name component, explicitly **not** a publish-target name; plan entries source names from `IdentityIndex::native_name` because a Case D package can have two divergent native names. |
| 23 | §M.12.2 | `ReleaseEntry.changelog_section` is `Option<String>`, produced by reading the written `CHANGELOG.md` back (§CL.7.1). |

### M.17 Fixture obligations

`callisto-fixtures` must carry, for this crate specifically (§12.6's "broader than JSON shape
alone"):

1. **`PackageId::parse` corpus** — `@myorg/foo`, `cargo/foo`, `npm/@myorg/foo`,
   `maven/org.example:foo-core`, `foo`, `not-an-ecosystem/foo`, `cargo/`, `""`. This is the
   same failure family as §6.1's unquote-before-split rule and gets the same fixture
   treatment.
2. **Tag round-trip corpus** — for each of `{default template, "foo@{version}", "v{version}",
   "release/{version}"}`: rendered tag, derived glob, and extraction against a tag list that
   includes a sibling package's tags (`foo@1.0.0`, `foo-bar@2.0.0`) to prove the parse step is
   the discriminator, not the glob (§M.9.3 step 4).
3. **Tag template rejections** — one fixture per `TagTemplateError` variant, `{name}`
   included, since that is the placeholder users will try.
4. **Severity aggregation** — `max_of` over every subset of the four values, asserting
   `None < Patch < Minor < Major` rather than declaration order.
5. **`Version` equality and comparison** — `1.2.3` vs `1.2.3+abc` (equal under SemVer
   precedence, **not** equal under `Version`'s `PartialEq`); a `SemVer`/`Pep440` pair asserting
   `compare` returns `Err(GrammarMismatch)` and `partial_compare` returns `None`.
6. **Plan/report golden files** — one per `Report::COMMAND`, at `schemaVersion` 1, including
   an empty-arrays plan (to prove mandatory arrays serialize as `[]` and never vanish) and a
   plan with a custom registry (to prove `publishTo` stays a string).
7. **Auto-trait assertions** — §M.1.5's compile test.
8. **`wasm32-wasip1` fixture run** — the whole suite under `wasmtime` with only the workspace
   root preopened, as a CI job from v0.1, before `callisto-moon` exists (§0.1 rule 2, §13
   inv. 26). This crate is the cheapest place to prove the harness works, since it has no I/O
   to sandbox.

---

## 3. `callisto-format`

**Purpose.** The byte-compatible parser/writer for `@changesets/cli`'s on-disk formats —
`.changeset/*.md` and `pre.json` — plus `bump_version`, the SemVer arithmetic that must match
`@changesets/cli` exactly.

**License:** MIT OR Apache-2.0 (§16). **Milestone:** v0.1 (§17).

Traces to §5.1 (`Severity`), §6 (changeset file format), §6.2 (`bump_version`), §6.4/§8
(`pre.json`), §15/§16 (crate layout and license tier), §13 invariants 1–2, §7.7 (the
`Versioning` trait), and §12.6 (fixture strategy).

### F.1 Purpose and scope

This crate owns three things:

- The changeset markdown format: frontmatter parsing/writing, the quoted-vs-bare name grammar
  (§6.1), and the empty-changeset validity rule.
- `bump_version` and the `Versioning` trait (§6.2, §7.7) — the pure version arithmetic
  `@changesets/cli` compatibility is measured against.
- `pre.json`'s byte shape (§6.4, §8).

**It is "the primitive worth spreading" (§15), and that shapes its dependency posture.** A
team that wants a Rust changesets-format implementation without buying into callisto's
graph/cascade/manifest opinions should be able to depend on this crate plus
`callisto-model` and nothing else. That is the whole of the boundary being defended — not
"zero dependencies," which §M.4.5 shows is unachievable once `Severity` and `Version` have a
single home.

### F.2 Dependencies

| Edge | Kind | Why |
|---|---|---|
| `callisto-format → callisto-model` | normal | `Severity`, `Version`, `VersionGrammar`, `Diagnostic`, `ModelError` (§M.4.5, §M.14's `format` column) |
| `callisto-format → callisto-fixtures` | **dev** | the byte-compat corpus (§F.9). Dev-only, so invisible in `cargo metadata --no-dev-dependencies` and in anything a published consumer resolves. `callisto-fixtures` is enabled here with **default features only** (model-tier data), never the `graph` feature, which is what keeps the dev-dependency cycle to the shape Cargo permits (§CF.2). |

**Deliberately absent:** `callisto-graph`, `callisto-manifests`, `callisto-conventional`,
`callisto-changelog`, `callisto-cli`, `callisto-moon`, and anything moon-related. Enforced by
the same `xtask dep-audit` CI job §13 invariant 26 uses for the moon-free rule (§G.1.7), not
by convention.

**Reconciliation note.** An earlier draft of this crate's spec proposed the opposite of
§M.4.5 — that `Severity` be defined *here* and re-exported by `callisto-model`, with this
crate having zero `callisto-*` dependencies of any kind. That is incompatible with
`callisto-model` depending on nothing (which every other crate's spec assumes) and with
`bump_version` operating on `callisto-model`'s grammar-tagged `Version` (§M.4.1, §M.15).
`callisto-model` wins, per the general rule that shared types take their canonical shape from
the crate everything else depends on. §11 records the reversal.

```toml
[package]
name = "callisto-format"
version = "0.1.0"
edition = "2021"
license = "MIT OR Apache-2.0"
description = "Byte-compatible parser/writer for @changesets/cli's changeset markdown and pre.json file formats."
repository = "https://github.com/orin-dx/callisto"

[dependencies]
callisto-model = { path = "../callisto-model" }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
# Order-preserving map — see §F.7's SPEC DECISION for why this replaces the more obvious
# BTreeMap/HashMap choice for `PreState::initial_versions`.
indexmap = { version = "2", features = ["serde"] }
thiserror = "1"

[dev-dependencies]
callisto-fixtures = { path = "../callisto-fixtures", default-features = false }
```

The crate ships both `LICENSE-MIT` and `LICENSE-APACHE`, per §16's dual-license posture for
the primitive-tier crates. `semver` is not a direct dependency: every version value crossing
this crate's API is a `callisto_model::Version`, and the SemVer arithmetic in §F.6 operates
through that type's accessors.

### F.3 Dependency and I/O posture

> `[SPEC DECISION, not in 00-design.md: `callisto-format`'s public API is filesystem-free —
> every function operates on in-memory `&str`/owned values, never a `Path`.]` §6.1's
> "Filenames arbitrary, sorted for deterministic read order" describes a directory-listing
> concern, not a single-file parse concern; nothing in §6 requires this crate itself to walk
> `.changeset/`. Keeping I/O out is the smallest decision consistent with two things the
> design doc already commits to: the compute/apply split (§0.1 rule 3 — "every command is a
> pure function… plus a separate step that touches disk") and this crate's primitive status,
> which would otherwise force every consumer wanting a different directory-walk strategy
> (in-memory fixtures, a real filesystem, a virtual-path-mapped WASM sandbox) to work around
> a decision this crate has no business making for them.

Concretely: `callisto-graph` reads `.changeset/*.md` files off disk in filename-sorted order
(§G.6.1) and hands each file's *contents* to `parse_changeset`; `callisto-cli` writes the
string `write_changeset` returns (§CLI.6.1). Because this crate performs no I/O, it trivially
satisfies §0.1 rule 2's `wasm32-wasip1` + `wasmtime` CI requirement; it is in that job for
completeness and to catch a future PR accidentally adding I/O, not because it is expected to
be the thing that fails it.

### F.4 Module layout

```
callisto-format/
└── src/
    ├── lib.rs             # crate docs, public re-exports
    ├── changeset/
    │   ├── mod.rs          # Changeset, Entry, ParseError, WriteError,
    │   │                   #   parse_changeset, write_changeset
    │   └── frontmatter.rs  # private: line-level name/severity tokenizer, quoting predicate
    ├── bump.rs             # Versioning, SemVerVersioning, bump_version, BumpError
    └── pre.rs              # PreState, PreMode, PreJsonError, parse_pre_json, write_pre_json
```

```rust
// lib.rs
//! Byte-compatible reader/writer for `@changesets/cli`'s file formats.
//!
//! - The changeset markdown format (§6.1): frontmatter parsing/writing, the quoted-vs-bare
//!   name grammar, the empty-changeset validity rule.
//! - `bump_version` (§6.2) and the `Versioning` trait (§7.7).
//! - `pre.json`'s byte shape (§6.4, §8).
//!
//! Depends only on `callisto-model` among callisto crates, and performs no I/O — see §F.2
//! and §F.3 of the crate spec for why both boundaries are deliberate.

pub mod bump;
pub mod changeset;
pub mod pre;

pub use bump::{bump_version, BumpError, SemVerVersioning, Versioning};
pub use changeset::{parse_changeset, write_changeset, Changeset, Entry, ParseError, WriteError};
pub use pre::{parse_pre_json, write_pre_json, PreJsonError, PreMode, PreState};

/// Re-exported for consumers that want the changesets primitive without also naming
/// `callisto-model`. Canonically defined there (§M.5).
pub use callisto_model::{Severity, SeverityParseError};
```

### F.5 The changeset markdown format — `changeset`

#### F.5.1 Public types

```rust
/// One parsed `.changeset/*.md` file (§6.1's shape): a frontmatter block of
/// `name: severity` entries, followed by a free-text summary.
///
/// Deliberately does not carry a filename or path — this crate is filesystem-free (§F.3); a
/// caller that reads files off disk attaches the filename itself for sort ordering and error
/// context (`callisto-graph`'s `LoadedChangeset`, §G.6.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Changeset {
    pub entries: Vec<Entry>,
    pub summary: String,
}

/// One `"name": severity` frontmatter line, after quote resolution.
///
/// `name` is the raw string as written in the changeset — resolving it to a `PackageId`
/// (bare vs. `ecosystem/name`-prefixed, §5.4) is a workspace-aware operation this crate
/// cannot perform (it does not know what packages exist) and does not attempt; that
/// resolution is `callisto-graph`'s (§G.6.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub severity: Severity,
}

impl Changeset {
    /// Convenience wrapper around [`write_changeset`].
    pub fn to_markdown(&self) -> Result<String, WriteError> { write_changeset(self) }
}
```

#### F.5.2 Parse and write entry points

```rust
/// Parses one `.changeset/*.md` file's contents.
///
/// Grammar (§6.1): a `---`-delimited frontmatter block starting on line 1, each non-blank,
/// non-comment line inside it shaped `<name>: <severity>`, followed by the file's remaining
/// content as `summary` (trimmed). `#`-comment lines inside the frontmatter block are
/// skipped. Blank lines inside the frontmatter block are skipped. CRLF line endings are
/// normalized to LF before parsing (§F.5.3); this affects only how the input is read, never
/// what `write_changeset` emits.
pub fn parse_changeset(source: &str) -> Result<Changeset, ParseError>;

/// Serializes a [`Changeset`] back to `.changeset/*.md` bytes.
///
/// Names are quoted only when necessary (§6.1: "quoted-when-necessary on write") — see
/// `needs_quoting` (§F.5.4) for the exact rule. Severities are always written lowercase
/// (`Severity`'s `Display` impl, §M.5). Output always uses `\n` line endings and ends with a
/// single trailing newline after the summary, matching `@changesets/cli`'s own writer.
pub fn write_changeset(changeset: &Changeset) -> Result<String, WriteError>;
```

#### F.5.3 Line endings

> `[SPEC DECISION, not in 00-design.md: input CRLF line endings are normalized to LF before
> parsing; output is always LF-terminated.]` Neither §6 nor §13 says anything about line
> endings. Since P1's byte-compatibility target (`@changesets/cli`, a Node tool that emits LF
> on every platform including Windows) writes LF, and since silently accepting CRLF on read
> without normalizing would let a Windows-edited changeset round-trip into a *different* byte
> sequence than the reference tool would produce for equivalent content, LF-write is the only
> choice consistent with P1; normalizing CRLF on read is the smallest addition that makes the
> parser tolerant of a plausible real-world input without weakening that guarantee.

Note the deliberate asymmetry with `callisto-manifests`, which *preserves* a `package.json`'s
existing CRLF (§CM.5.1). The rule differs because the target differs: `package.json` is a file
callisto edits in place and whose formatting belongs to the user, while `.changeset/*.md` is a
file callisto and `@changesets/cli` both author from scratch and must agree on byte-for-byte.

#### F.5.4 The frontmatter grammar, precisely (§13 invariant 1)

```rust
// changeset/frontmatter.rs (private to the crate; parse_changeset's implementation detail,
// spelled out here because the algorithm IS the spec for invariant 1, not incidental code.)

/// The name half of one frontmatter line, before severity resolution.
enum NameToken<'a> {
    Bare(&'a str),
    /// Owned because a quoted name's content is copied out from between the delimiting
    /// quotes, independent of any surrounding text.
    Quoted(String),
}

/// Line-relative failure — the caller (`parse_changeset`) attaches the absolute line number
/// and promotes this into the corresponding [`ParseError`] variant.
enum LineError {
    UnclosedQuotedName,
    AmbiguousNameQuoting { raw: String },
    MissingSeparator { raw: String },
    EmptyName,
    InvalidSeverity(SeverityParseError),
}

/// Splits one frontmatter line into its name token and the unparsed remainder (starting at
/// the separator `:`). This is the function §13 invariant 1 ("Frontmatter parser unquotes
/// before splitting on `:`, not after — P5") names.
///
/// For a **quoted** name, the closing `"` is located by scanning forward for the matching
/// delimiter — never by searching for `:` — before anything about the remainder of the line
/// is inspected. This is "unquoting before splitting" made concrete: a name that itself
/// contains a `:` (Maven's `groupId:artifactId` form, e.g. `"maven/org.example:foo-core"`,
/// §5.4) is never mis-split, because the colon search for the *separator* only ever runs on
/// the remainder *after* the name's own boundary has already been resolved by quote-matching.
/// The reference `knope-dev/changesets` crate does this the other way — colon-splitting the
/// raw, still-quoted line first — which is exactly the bug §6.1 cites as the reason callisto
/// does not depend on that crate.
///
/// For a **bare** (unquoted) name, there is nothing to unquote, so `rsplit_once(':')` over
/// the whole line is applied directly and is safe: an unquoted package name legitimately
/// containing a `:` is not representable in this grammar at all (bare Maven-style identity
/// must be quoted, since an unquoted `:` is exactly the separator this function is looking
/// for) — the parser has no ambiguity to resolve for the bare case, only the quoted case.
fn split_name_and_rest(line: &str) -> Result<(NameToken<'_>, &str), LineError> {
    let trimmed = line.trim_start();
    if let Some(after_quote) = trimmed.strip_prefix('"') {
        let end = after_quote.find('"').ok_or(LineError::UnclosedQuotedName)?;
        let name = &after_quote[..end];
        let rest = after_quote[end + 1..].trim_start();
        let rest = rest
            .strip_prefix(':')
            .ok_or_else(|| LineError::AmbiguousNameQuoting { raw: line.to_string() })?;
        Ok((NameToken::Quoted(name.to_string()), rest))
    } else {
        let (name, rest) = trimmed
            .rsplit_once(':')
            .ok_or_else(|| LineError::MissingSeparator { raw: line.to_string() })?;
        Ok((NameToken::Bare(name.trim_end()), rest))
    }
}

/// Parses one frontmatter line (already known non-blank, non-`#`-comment) into an [`Entry`].
fn parse_entry_line(line: &str) -> Result<Entry, LineError> {
    let (token, rest) = split_name_and_rest(line)?;
    let name = match token {
        NameToken::Bare(s) => s.to_string(),
        NameToken::Quoted(s) => s,
    };
    if name.is_empty() {
        return Err(LineError::EmptyName);
    }
    let severity = rest
        .trim()
        .parse::<Severity>()
        .map_err(LineError::InvalidSeverity)?;
    Ok(Entry { name, severity })
}

/// §6.1: "quoted-when-necessary on write." Conservative by construction — over-quoting is
/// lossless (a quoted `cargo/foo` still reads back as `cargo/foo`), under-quoting corrupts
/// output, so every character class below that could make a bare scalar ambiguous or
/// YAML-invalid triggers quoting rather than being reasoned about case by case at each call
/// site.
fn needs_quoting(name: &str) -> bool {
    let Some(first) = name.chars().next() else {
        return true; // empty name — write_changeset rejects this separately (WriteError::EmptyName)
    };
    matches!(
        first,
        '@' | '`' | '"' | '\'' | '&' | '*' | '!' | '|' | '>' | '%' | '#' | '-' | '?' | ':'
            | ',' | '[' | ']' | '{' | '}' | ' '
    ) || name.contains(':')
        || name.contains('#')
        || name.ends_with(' ')
}
```

> `[SPEC DECISION, not in 00-design.md: the exact character set in `needs_quoting`.]` §6.1
> states the *outcome* ("matches `@changesets/cli` output") for the one worked example it
> gives (`@myorg/foo` quoted, `cargo/foo` bare) but not the general predicate. The set chosen
> here is YAML's own reserved/indicator characters for plain scalars — `@` and `` ` `` are
> reserved outright by the YAML spec (which is why a bare npm scoped name is invalid YAML and
> must be quoted), and the rest are flow/block indicator characters that are ambiguous or
> illegal to lead a plain scalar with. A literal `:` anywhere in the name (not just leading)
> also forces quoting, independent of position, because an unquoted `:` is this grammar's own
> separator token (§5.4's Maven `groupId:artifactId` form is the concrete case this exists
> for).

#### F.5.5 `ParseError` — every failure mode §6 names

```rust
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ParseError {
    /// The file does not open with a `---` delimiter on line 1 at all — not "frontmatter
    /// present but malformed," but no frontmatter block recognized whatsoever.
    #[error("changeset does not start with a `---` frontmatter delimiter on line 1")]
    MissingFrontmatterStart,

    /// A `---` opened on line 1 but no matching closing `---` line was ever found.
    #[error("frontmatter opened with `---` on line 1 but was never closed with a matching `---`")]
    UnclosedFrontmatter,

    /// A quoted name's opening `"` has no matching closing `"` on the same line.
    #[error("line {line}: quoted name is never closed with a matching `\"`")]
    UnclosedQuotedName { line: usize },

    /// The closing `"` of a quoted name is not immediately followed (after optional
    /// whitespace) by the separator `:` — i.e. there is trailing content between the quote
    /// and the colon that callisto cannot attribute to either the name or the severity. Named
    /// "ambiguous," not "malformed," because the raw text is syntactically parseable multiple
    /// ways and this crate refuses to guess (P5) rather than silently picking one.
    #[error("line {line}: quoted name `{raw}` is followed by unexpected content before the `:` separator")]
    AmbiguousNameQuoting { line: usize, raw: String },

    /// A bare (unquoted) line contains no `:` at all.
    #[error("line {line}: no `:` separator found in {raw:?}")]
    MissingSeparator { line: usize, raw: String },

    /// The name resolved to the empty string (`"": minor`, or `: minor` with a bare empty
    /// name before the colon).
    #[error("line {line}: package name is empty")]
    EmptyName { line: usize },

    /// The severity token is not one of `major | minor | patch | none` (case-insensitive).
    #[error("line {line}: invalid severity for package {name:?}: {source}")]
    InvalidSeverity {
        line: usize,
        name: String,
        #[source]
        source: SeverityParseError,
    },

    /// The same (raw, pre-`PackageId`-resolution) name appears twice in one changeset's
    /// frontmatter.
    #[error("line {line}: package {name:?} is named more than once in this changeset's frontmatter (first on line {first_line})")]
    DuplicateEntry {
        line: usize,
        first_line: usize,
        name: String,
    },

    /// §6.1: "Empty frontmatter valid iff summary is non-empty." Zero entries and an
    /// empty (whitespace-only) summary together mean the changeset has nothing for
    /// `callisto version` to act on and nothing for a human to read — invalid.
    #[error("changeset has no frontmatter entries and an empty summary")]
    EmptyChangeset,
}
```

> `[SPEC DECISION, not in 00-design.md: duplicate names within one changeset file are a hard
> parse error (`ParseError::DuplicateEntry`), not last-value-wins.]` §6 does not address this
> case. `@changesets/cli` parses frontmatter as YAML, where a duplicate mapping key's
> behaviour depends on the underlying YAML library's own tolerance (commonly
> last-value-wins, not itself a spec-guaranteed behaviour) — so there is no single
> well-defined "the byte-compatible answer" to copy here, and P1's textual scope is the file
> *format*, not this edge case's resolution rule. Given that, this crate applies P5 instead: a
> duplicate name in one changeset is almost certainly a copy-paste mistake (a human meant to
> name two different packages and typo'd), and silently keeping one interpretation risks
> masking exactly that mistake rather than surfacing it. Flagged for re-litigation if a real
> `@changesets/cli`-authored fixture is ever found that relies on duplicate-key tolerance.

Note the interaction with §5.4: two *different* names that resolve to the same `PackageId`
(`"cargo/foo": patch` and `"npm/foo": minor` on a Case D package) are **not** a duplicate here
and must not be — §5.4 specifies exactly that shape and expects it to aggregate by max. This
error is about literal string equality of the raw names, before any resolution; the
`PackageId`-level aggregation happens in `callisto-graph` (§G.6.1), which is the only layer
that knows the two names are one package.

`line` fields are 1-indexed, absolute line numbers within the source string passed to
`parse_changeset` — not relative to the frontmatter block's start — so a caller can point an
editor or error message directly at the offending line without re-deriving an offset.

#### F.5.6 `WriteError`

```rust
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum WriteError {
    /// Mirrors `ParseError::EmptyChangeset` — refuses to write a file that
    /// `parse_changeset` would immediately reject, per P3 (idempotence: a value that cannot
    /// round-trip should not be produced in the first place).
    #[error("cannot write changeset: no entries and an empty summary")]
    EmptyChangeset,

    /// `entries[index]`'s name is the empty string.
    #[error("entry {index} has an empty package name")]
    EmptyName { index: usize },

    /// `entries[index]`'s name contains a literal `"`. This grammar defines no escaping
    /// convention for quotes inside quoted names (§6.1 gives none, and no real npm/Cargo/
    /// Maven package identity can contain a `"`), so there is no byte-compatible way to
    /// write this name at all — refusing loudly (P5) beats guessing at an escape scheme
    /// `@changesets/cli` does not share.
    #[error("entry {index} name {name:?} contains a literal `\"`, which cannot be written (no escaping convention is defined for this grammar)")]
    NameContainsQuote { index: usize, name: String },
}
```

### F.6 Version arithmetic — `bump`

#### F.6.1 The `Versioning` trait

§7.7 is explicit: "`bump_version` is a method on a per-ecosystem `Versioning` trait, not a
free function." §M.15 assigns both to this crate.

```rust
/// §7.7. One implementation per version grammar. `SemVer` is the only one with a body in the
/// committed v0.1–v0.4 scope, because both committed ecosystems are SemVer (§2.2); the trait
/// exists so that adding PEP 440 or Maven is "one grammar impl" (P4) rather than a rewrite.
///
/// There is **no user-facing override key** for grammar selection (§Q5.3's "on per-ecosystem
/// grammar overrides"): the impl is selected by `Ecosystem::version_grammar`, full stop.
pub trait Versioning: Send + Sync {
    fn grammar(&self) -> VersionGrammar;
    fn bump(&self, current: &Version, severity: Severity) -> Result<Version, BumpError>;

    /// §8's pre-release arithmetic — the `pre.0 → pre.1 → pre.2` monotonic counter. See
    /// §F.6.3 for the algorithm and for why it is a `Versioning` method rather than a free
    /// function or a `callisto-graph` computation.
    fn bump_prerelease(
        &self,
        base: &Version,
        severity: Severity,
        tag: &str,
        current: &Version,
    ) -> Result<Version, BumpError>;
}

/// The SemVer implementation. Its `bump` body *is* `bump_version` below.
pub struct SemVerVersioning;

/// Selects the implementation for a grammar. `None` for a declared-but-unimplemented grammar
/// (§M.4.1's `Pep440`/`Maven`), which callers turn into a specific error rather than a panic.
///
/// **The `Option` is load-bearing and every caller must handle it** — `callisto-graph`'s
/// `bump_target` (§G.7.4) turns `None` into `BumpError::UnsupportedGrammar` before any write
/// happens, rather than panicking partway through a mutation phase.
pub fn versioning_for(grammar: VersionGrammar) -> Option<&'static dyn Versioning>;
```

#### F.6.2 `bump_version`

```rust
/// Byte-exact match to `@changesets/cli`'s version bump semantics (§6.2, §13 invariant 2).
/// No exceptions, no config path reaches this function, ever — §7.1's opt-in
/// `pre-major-inference` remap (and any future policy layer) operates one level up, on the
/// *severity* an `Auto`-trigger package infers, never on this function. If a future change
/// adds a parameter here to support a remap, it has violated P1/invariant 2; the correct
/// place for that logic is `callisto-conventional` (§C.4).
///
/// - `major` → `{major + 1}.0.0` — deliberately **not** remapped for `0.x` versions
///   (`0.5.2` + `major` → `1.0.0`, never `0.6.0`). This is the single most load-bearing
///   behaviour in this function; see `02-library-vs-moon-decision.md`'s "0.x remap rigidity"
///   section for the full argument against ever softening it here.
/// - `minor` → `{major}.{minor + 1}.0`
/// - `patch` → `{major}.{minor}.{patch + 1}`
/// - `none` → `current`, unchanged — including any prerelease/build metadata, which is the
///   one case those are *preserved* rather than dropped.
///
/// Prerelease and build metadata are dropped on every real bump (`major`/`minor`/`patch`),
/// preserved only on `none` (§6.2).
///
/// **Fallible only for grammar reasons.** `Severity` is a closed four-variant enum and
/// `semver::Version`'s numeric components (`u64`) cannot realistically overflow from a single
/// increment, so `BumpError` has exactly one variant: the `Version` handed in was not parsed
/// under `VersionGrammar::SemVer`. An earlier draft made this function infallible by taking a
/// bare `semver::Version`; §M.4.1's grammar tagging is what turns that into a checkable
/// precondition instead of an unstated one, and returning `Result` is cheaper than a panic
/// path for a case a demand-gated ecosystem would make reachable.
pub fn bump_version(current: &Version, severity: Severity) -> Result<Version, BumpError>;

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum BumpError {
    #[error("bump_version requires a SemVer version; `{raw}` was parsed as {grammar:?}")]
    NotSemVer { raw: String, grammar: VersionGrammar },
    #[error("no versioning implementation exists for {grammar:?}")]
    UnsupportedGrammar { grammar: VersionGrammar },
}
```

Reference implementation, for the avoidance of doubt about the `0.x` case:

```rust
// SemVer body, expressed against callisto_model::Version's accessors.
match severity {
    Severity::Major => Version::semver(current.major() + 1, 0, 0),
    Severity::Minor => Version::semver(current.major(), current.minor() + 1, 0),
    Severity::Patch => Version::semver(current.major(), current.minor(), current.patch() + 1),
    Severity::None  => current.clone(),
}
```

#### F.6.3 `bump_prerelease` — §8's `pre.0 → pre.1 → pre.2` counter

> `[SPEC DECISION, not in 00-design.md: the pre-release counter arithmetic is a `Versioning`
> trait method (`bump_prerelease`) in `callisto-format`, with the exact algorithm below.]` §8
> specifies the *behaviour* ("subsequent `callisto version` runs bump from `initialVersions`
> (not on-disk), keeping `pre.0 → pre.1 → pre.2` monotonic") but no section assigned it an
> owner, and an earlier draft of this spec left it genuinely homeless: §F.7's SPEC DECISION
> disclaims implementing "§8's pre-release version-computation algorithm" *as a whole* —
> correctly, because the parts that need cross-package context (which changesets are already
> counted, which packages were touched, the rerun-safety check) are `callisto-graph`'s
> (§G.6.8) — while §G.7.4's `bump_target` only gestured at the arithmetic. The *arithmetic*
> itself has no cross-package context in it at all: it is a pure function of one base version,
> one severity, one tag, and one current version, and it is grammar-specific in exactly the
> way `bump` is (PEP 440's `aN`/`bN`/`rcN` pre-releases are a different spelling of the same
> counter, §7.7). That makes it a `Versioning` method by the same argument §7.7 uses for
> `bump`, and it keeps §6.2's rigidity claim intact: `bump_version` is unchanged, and this
> function *calls* it rather than parameterising it.

```rust
/// §8. Computes the next pre-release version for one package.
///
/// - `base`     — the package's `initialVersions` entry (§8: "bump from `initialVersions`,
///                not on-disk"), i.e. its version of record at `pre enter` time.
/// - `severity` — the aggregate severity for this run (§7.1), after every union.
/// - `tag`      — `pre.json`'s `tag` field, e.g. `"next"`.
/// - `current`  — the package's **on-disk** version, which is what carries the counter
///                reached by the previous `version` run in this cycle. `base` cannot carry
///                it: `initialVersions` never moves for the duration of a cycle.
fn bump_prerelease(&self, base: &Version, severity: Severity, tag: &str, current: &Version)
    -> Result<Version, BumpError>;
```

Algorithm (`SemVerVersioning`; the only implementation in committed scope):

```
 1. severity == Severity::None → return `current` unchanged, no counter advance. §6.2's
    "none is a no-op" applies here identically; a package with nothing new to record does not
    burn a pre-release number.

 2. release ← self.bump(base, severity)?            # §F.6.2, verbatim — no remap, no flag.
    `bump` already drops prerelease and build metadata on every real bump (§6.2), so `release`
    is a clean `X.Y.Z` regardless of what `base` carried.

 3. counter ←
      if `current` is a prerelease whose release part (`X.Y.Z`) equals `release`
         AND whose prerelease identifiers are exactly [`tag`, N] with N a non-negative
         integer:                       N + 1
      else:                             0

    The "release part equals `release`" guard is what makes the counter restart when a
    *later* changeset raises the cycle's target (e.g. `1.1.0-next.3` + a `major` changeset →
    `2.0.0-next.0`, not `2.0.0-next.4`): the counter counts pre-releases *of one target
    version*, which is the only reading under which `pre.0 → pre.1 → pre.2` is meaningful.
    A different `tag` in `current` (a previous cycle's leftovers) also restarts at 0.

 4. return `{release}-{tag}.{counter}` — SemVer prerelease identifiers, dot-separated, e.g.
    `1.1.0-next.0`. Build metadata is never emitted.
```

Monotonicity, which is the property §8 actually asks for, follows from steps 2–3 rather than
being asserted: within one target version the counter strictly increases by construction, and
across target versions the release part strictly increases because `bump` is strictly
increasing for every non-`None` severity — so the SemVer precedence order of successive
results is strictly increasing in both cases. Idempotence across repeated `version` runs is
**not** this function's job and must not be read into it: §8 delivers that by not re-consuming
changesets already recorded in `pre.changesets` (§G.6.8), so this function is simply not
called for a package with nothing new.

`tag` is validated by the caller, not here — `callisto-graph` rejects a tag that is not a
legal SemVer prerelease identifier (`[0-9A-Za-z-]+`, and not a purely numeric one, which
would make `{tag}.{counter}` ambiguous) at `pre enter` time (§CLI.6.4).

### F.7 `pre.json` — `pre`

> `[SPEC DECISION, not in 00-design.md: this crate parses and writes `pre.json`'s byte-shape
> only; beyond §F.6.3's `bump_prerelease` arithmetic it does not implement the pre-release
> *orchestration* §8 describes — deciding which packages bump, sourcing each one's `base` from
> `initialVersions`, and tracking which changesets are already counted.]` That orchestration
> needs
> cross-package aggregation context this crate cannot have — which packages' severities were
> touched this run, each package's current on-disk version and existing prerelease counter,
> the rerun-safety check (§7.6 step 1). All of that lives in `callisto-graph` (§G.6.8). What
> `callisto-format` owns is narrower and self-contained: turning a `pre.json` file's bytes
> into a typed `PreState` and back, so that everything above it has a single, tested,
> byte-shape-correct serialization to build on instead of each caller hand-rolling
> `serde_json::Value` field access.

```rust
/// Byte-shape-compatible with `@changesets/cli`'s `pre.json` (§6.4, §8): same filename
/// convention (`pre.json`, at the `.changeset/` root — the filename itself is a caller's I/O
/// concern per §F.3, not something this type carries), same camelCase field names,
/// 2-space-indented JSON, trailing newline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreState {
    pub mode: PreMode,
    pub tag: String,
    /// Snapshot of every affected package's version at the moment `pre enter` ran (§8) —
    /// the baseline subsequent `pre.N` bumps compute from, never itself compared against
    /// on-disk state for alignment checking (§8: "`initialVersions` is a bump-computation
    /// input only, never an alignment-check input" — that rule governs how `callisto-graph`
    /// uses this field, not anything this crate enforces structurally, since this crate has
    /// no alignment-check logic to misuse it in).
    ///
    /// Keys are package-name strings exactly as written in the file, not `PackageId`s —
    /// resolving a name to an identity is workspace-aware and therefore not this crate's job
    /// (§F.5.1's note on `Entry::name`, applied to the same problem).
    pub initial_versions: indexmap::IndexMap<String, Version>,
    /// Changeset IDs (filenames, without the `.md` extension) already consumed by a
    /// `pre.N` bump during this pre-release cycle — read on each `version` run so a package
    /// with no *new* changeset since the last pre-bump is not re-incremented (§8).
    pub changesets: Vec<String>,
}

impl PreState {
    /// Pure constructor for `callisto pre enter <tag>` (§8, §CLI.6.4). The caller supplies
    /// the snapshot; this crate does not read manifests.
    ///
    /// Takes an iterator rather than an `IndexMap` so a caller can hand over the
    /// already-ordered `Vec<(String, Version)>` that `Workspace::initial_versions` (§G.11)
    /// produces without naming `indexmap` itself — `callisto-cli` would otherwise need a
    /// dependency on it purely to call this function. Insertion order is preserved verbatim
    /// (§F.7's `IndexMap` decision); a duplicate key keeps the **first** value, which is
    /// unreachable from `initial_versions`' own uniqueness check but is defined rather than
    /// left to the map's default.
    pub fn entering(tag: impl Into<String>,
                    initial_versions: impl IntoIterator<Item = (String, Version)>) -> Self;
    /// Pure transition for `callisto pre exit` (§8): flips `mode` to `Exit` without touching
    /// `tag`, `initial_versions`, or `changesets`, and without deleting anything — deletion
    /// happens on the *next* `version` run, which is `callisto-graph`'s step (§G.6.8).
    pub fn exiting(self) -> Self;
}

/// `mode: "pre" | "exit"` (§8's literal field values — `callisto pre enter` sets `"pre"`;
/// `callisto pre exit` flips it to `"exit"` without deleting the file).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreMode { Pre, Exit }
```

> `[SPEC DECISION, not in 00-design.md: `initial_versions` is an order-preserving map
> (`indexmap::IndexMap`), not `BTreeMap`/`HashMap`.]` §6.4 calls the format
> "byte-*shape*-compatible" (filename, field names, 2-space indent, trailing newline) rather
> than claiming literal byte-identity the way §6.2 does for `bump_version`, so key ordering is
> not unambiguously specified either way. `@changesets/cli` writes `initialVersions` as a
> plain JS object, whose key order is insertion order, not alphabetical — a `BTreeMap` would
> silently reorder entries into alphabetical order on every write, which is a needless,
> avoidable divergence from what a byte-diff against a real `@changesets/cli`-authored
> `pre.json` would show. `IndexMap` costs one extra dependency and preserves whatever order
> the caller (or the original parse) produced.

```rust
/// Parses `pre.json`'s contents into a [`PreState`].
///
/// Hand-rolled field-level validation rather than a single `serde::Deserialize` derive on
/// `PreState` directly, so that a malformed file produces a specific, actionable
/// [`PreJsonError`] variant instead of an opaque `serde_json::Error` — consistent with P5
/// (structural fixes, including error quality, over "read the stack trace").
pub fn parse_pre_json(input: &str) -> Result<PreState, PreJsonError>;

/// Serializes a [`PreState`] to `pre.json`'s bytes: 2-space indent, camelCase field names,
/// single trailing newline. Infallible — `PreState`'s fields are already well-typed
/// (`PreMode`, `Version`), so there is nothing left that could fail at serialization time.
pub fn write_pre_json(state: &PreState) -> String;
```

```rust
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PreJsonError {
    /// Not valid JSON at all, or valid JSON that is not a top-level object (e.g. an array or
    /// bare string) — nothing field-level to diagnose. Carries the rendered message rather
    /// than a `serde_json::Error` so the enum stays `Clone + PartialEq` for fixtures (§1.5 of this document).
    #[error("pre.json is not a valid JSON object: {message}")]
    Malformed { message: String },

    /// One of `mode`, `tag`, `initialVersions`, `changesets` is missing from the object.
    #[error("pre.json is missing required field {field:?}")]
    MissingField { field: &'static str },

    /// A field is present but the wrong JSON type (e.g. `changesets` is a string instead of
    /// an array, or `initialVersions` is an array instead of an object).
    #[error("pre.json field {field:?} has the wrong type")]
    WrongFieldType { field: &'static str },

    /// `mode` is present and is a string, but not exactly `"pre"` or `"exit"`.
    #[error("pre.json has mode {found:?}, expected \"pre\" or \"exit\"")]
    InvalidMode { found: String },

    /// One `initialVersions` entry's value is not a valid version under the assumed grammar.
    #[error("pre.json initialVersions[{package:?}] = {raw:?} is not a valid version: {source}")]
    InvalidInitialVersion {
        package: String,
        raw: String,
        #[source]
        source: VersionParseError,
    },

    /// A `changesets` array element is not a string.
    #[error("pre.json changesets[{index}] is not a string")]
    InvalidChangesetId { index: usize },
}
```

`InvalidInitialVersion`'s scope note: values are parsed under `VersionGrammar::SemVer`,
matching §M.4.2's wire-format rule, because the committed core (§2.2) is Rust/npm — both
SemVer. A demand-gated ecosystem with a non-SemVer version grammar (§7.7) would need this
crate's `pre.json` support to widen; that is explicitly out of scope until such an ecosystem
is built, matching §2.2's own scope discipline rather than speculatively generalizing now.

### F.8 What this crate deliberately does not own

| Concept | Owner | Why not here |
|---|---|---|
| `Severity`, `Version`, `VersionGrammar`, `Diagnostic` | `callisto-model` | §M.4.5 — one home for a shared type, and the crate everything depends on is where it goes. |
| Directory walking, filename sorting, changeset file naming | `callisto-graph` (read, §G.6.1) / `callisto-cli` (write, §CLI.6.1) | §F.3 — this crate is filesystem-free. |
| Resolving a changeset `Entry::name` to a `PackageId` | `callisto-graph` (§G.6.1) | Workspace-aware; §5.4's resolution order needs to know what packages exist. |
| §8's pre-release **orchestration** — which packages bump, sourcing `base` from `initialVersions`, tracking already-counted changesets | `callisto-graph` (§G.6.8) | §F.7's SPEC DECISION — needs cross-package aggregation context. The per-package pre-release *arithmetic* is this crate's, as `Versioning::bump_prerelease` (§F.6.3). |
| `.changeset/config.json` translation (§18 Q4) | `callisto-graph`'s `init` (§G.11), v0.4 | That file encodes *settings*, which §14's `callisto.toml` schema holds; it is not one of the two formats P1 guarantees compatibility for. |

### F.9 Fixture obligations

Per §12.6's principle — "a canary only catches what it fixtures" — and this crate's specific
byte-compatibility obligation (P1), the test suite is built entirely around fixtures rather
than hand-written example inputs, sourced from `callisto-fixtures` (§CF).

**Corpus layout, under `callisto-fixtures/data/`:**

- `changesets/valid/*.md` — real `.changeset/*.md` files vendored verbatim from
  `@changesets/cli`'s own test suite, plus hand-authored additions covering shapes the
  reference suite does not happen to exercise: ecosystem-prefixed names (`cargo/foo`),
  Maven-style colon identity, `#`-comment lines, empty-frontmatter-with-summary, multiple
  entries with mixed quoted/bare names, `none` severity, mixed-case severities.
- `changesets/invalid/*.md` plus a `cases.toml` mapping each invalid fixture's filename to
  the exact `ParseError` variant name it must produce — one fixture per variant in §F.5.5, at
  minimum. This is the operative test, not `assert!(result.is_err())`: a refactor that changes
  *which* error fires for a given malformed input (e.g. silently reclassifying an
  `AmbiguousNameQuoting` case as `MissingSeparator`) fails CI, per P7's "every boundary is
  explicit and testable."
- `bump-version/table.json` — `(current, severity, expected)` triples pulled from
  `@changesets/cli`'s own bump-version test suite, explicitly including the `0.x`-no-remap
  cases (`0.5.2` + `major` → `1.0.0`) and prerelease/build-metadata drop-vs-preserve cases.
- `pre/*.json` — real `pre.json` snapshots captured from running the reference CLI through
  `enter`/multiple `version`/`exit` sequences, covering both `mode` values, multi-package
  `initialVersions`, and the mid-cycle synthesized-member case (§8, §G.8.3).

**Test classes:**

1. **Round-trip byte-identity** — for every fixture in `changesets/valid/` and `pre/`:
   `parse` then re-`write` must reproduce the original bytes exactly. This is the literal
   test of the "byte-compatible" claim, not an assertion of it.
2. **Negative-fixture variant matching** — for every fixture in `changesets/invalid/`:
   `parse_changeset` must return the specific `ParseError` variant `cases.toml` names, not
   merely `Err(_)`.
3. **`bump_version` golden table** — exact `Version` equality (§M.4.2's field-level `PartialEq`,
   not SemVer precedence equality, which ignores build metadata) against every row.
4. **`bump_prerelease` golden table** (§F.6.3) — `(base, severity, tag, current, expected)`
   rows covering: first bump of a cycle (`current` not a prerelease → `.0`); the monotonic
   advance (`1.1.0-next.0` → `1.1.0-next.1` → `.2`); the target-version-raised restart
   (`1.1.0-next.3` + `major` → `2.0.0-next.0`); a different `tag` in `current` (restarts at
   `.0`); `Severity::None` (returns `current` unchanged); and a `current` whose prerelease
   identifiers do not match `[tag, N]` at all (restarts at `.0`). These are behaviour
   fixtures, not byte-compat ones — `@changesets/cli`'s own pre-mode output is the
   cross-check where a real captured `pre/*.json` sequence supplies one.
5. **`pre.json` round-trip** — `parse_pre_json` then `write_pre_json` reproduces the original
   fixture bytes exactly (2-space indent, camelCase, trailing newline, and — because of the
   `IndexMap` decision — original key order).
6. **CRLF normalisation** — a CRLF-authored valid changeset parses to the same `Changeset` as
   its LF twin and writes back LF-only, proving §F.5.3's asymmetry is implemented rather than
   assumed.
7. **Provenance** — every vendored fixture (from `changesets/valid/` and `bump-version/`
   specifically, since those two claim actual upstream provenance rather than being
   hand-authored) carries a header comment or an adjacent `SOURCE.md` recording the
   `@changesets/cli` version/commit it was pulled from, so a future corpus refresh is an
   auditable diff, not silent drift.

**CI:** this crate's fixture suite runs both natively and under `wasm32-wasip1` via
`wasmtime` (§0.1 rule 2), as part of the workspace-wide core fixture job that exists from
v0.1 — for this crate specifically that is a low-risk inclusion (no I/O to fail under WASI's
sandboxed filesystem), but it stays in the enumerated matrix rather than being assumed safe,
consistent with P7.

### F.10 Index of `[SPEC DECISION]` flags

| # | Section | Decision |
|---|---|---|
| 1 | §F.2 | *(reversed at assembly)* `Severity` is canonically `callisto-model`'s and the crate edge runs `callisto-format → callisto-model` — see §M.4.5 and §11's reconciliation list. |
| 2 | §F.3 | `callisto-format`'s public API is filesystem-free. Directory walking, filename sorting, and file naming are the caller's job. |
| 3 | §F.5.3 | CRLF is normalized to LF on read; output is always LF-terminated. |
| 4 | §F.5.4 | The write-time name-quoting character set (`needs_quoting`'s exact predicate). |
| 5 | §F.5.5 | Duplicate *raw names* within one changeset file are a hard parse error, not last-value-wins — distinct from two prefixed names resolving to one `PackageId`, which is §5.4's supported shape. |
| 6 | §F.6.2 | `bump_version` returns `Result` with a single grammar-precondition error, rather than being infallible, because §M.4.1's `Version` is grammar-tagged. |
| 7 | §F.7 | `PreState::initial_versions` uses an order-preserving map (`IndexMap`), not `BTreeMap`. |
| 8 | §F.7 | This crate implements `pre.json`'s byte-shape plus §F.6.3's per-package pre-release arithmetic, not §8's cross-package pre-release orchestration. |
| 9 | §F.6.3 | The `pre.0 → pre.1 → pre.2` counter is `Versioning::bump_prerelease`, a trait method in this crate, with the algorithm pinned. |

---

## 4. `callisto-manifests`

**Purpose.** Per-ecosystem, format-preserving manifest read/write — the one crate in the
coordination core that does real filesystem I/O.

**License:** AGPL-3.0 (§16 — this is where per-ecosystem *behaviour* lives, which is exactly
the coordination-adjacent logic §16 draws the license line around, not the primitive/contract
tier). **Milestone:** v0.1 for Cargo + npm (§17).

### CM.0 Purpose and I/O posture

Every other core crate either does no I/O (`callisto-model`, `callisto-format`) or goes
through `CommandRunner` (§M.10); this crate is where `std::fs::read_to_string`/`std::fs::write`
calls actually live, format-preservingly, per ecosystem.

It still satisfies §0.1 rule 1 (zero moon in the dependency tree) and rule 2 (builds for
`wasm32-wasip1`, fixture suite passes under `wasmtime` with only the workspace root
preopened; §13 inv. 26 names this crate explicitly). `std::fs` compiles and works under WASI's
real filesystem API as long as every path stays inside the preopened root — §M.1.3's
workspace-root-relative discipline is what makes that true by construction — and neither
`toml_edit` nor `serde_json` needs anything WASI does not provide.

### CM.0.1 Dependencies

| Edge | Kind | Why |
|---|---|---|
| `callisto-manifests → callisto-model` | normal | `Manifest`'s entire vocabulary: `ManifestError`, `ManifestRole`, `ManifestFormat`, `ManifestDecl`, `Ecosystem`, `Version`, `VersionGrammar`, `VersionReq`, `DepSpec`, `DepKind`, `DependencyEntry`, `WorkspaceKind`, `PackageId` — used exactly as `callisto-model` declares them, never redefined |
| `toml_edit` | optional, `cargo` feature | format-preserving TOML (§7.6 step 3) |
| `serde_json` (`preserve_order`) | optional, `npm` feature | order-preserving JSON (§CM.5.1) |
| `callisto-fixtures` | **dev** | round-trip fidelity corpus (§CM.9) |

**Deliberately absent:** `callisto-graph` (graph construction, cascade, and group logic call
*into* this crate through the `Manifest` trait and the free functions below; nothing here
calls back), `callisto-format` (changeset parsing and manifest editing are unrelated concerns
that happen to both read/write files), and everything moon.

**What this crate deliberately does not own.** `ManifestError` (declared in `callisto-model`,
§M.13.2, precisely *because* this crate is feature-flagged per ecosystem and an error enum
declared here would vary in shape with the enabled feature set). `DepSpec`/`DepKind`/
`ManifestRole`/`ManifestFormat`/`Version`/`VersionReq` — all `callisto-model`'s, used here,
never redefined. `Coverage` — computed by `callisto-graph` (§G.7.3) from a `DepSpec` plus a
candidate `Version`; this crate never asks "does this spec cover that version," only "what
does this spec's raw string parse to" and "how do I rewrite this spec's raw string toward a
new version." `Package`, dependency-graph construction, cascade, groups — all
`callisto-graph`. Lockfile regeneration — a `CommandRunner` subprocess call driven by
`callisto-graph` at §7.6 step 9, never a `Manifest` trait method (§CM.2.4).

### CM.1 The `Manifest` trait

Reproduced exactly as §15 sketches it — no method added, removed, or renamed:

```rust
pub trait Manifest: Send + Sync {
    fn path(&self) -> &Path;
    fn ecosystem(&self) -> Ecosystem;

    /// Drives §7.6 steps 3/4/5's dispatch (canonical write vs. platform inherit vs.
    /// `optionalDependencies` update) — the axis §5.2 calls callisto's key differentiator.
    /// Purely descriptive at the impl level: **no method below branches on `role()`**. A
    /// `Platform`-role `package.json`'s `write_version` is the identical code path as a
    /// `Canonical`-role one (§CM.6). `role()` exists for `callisto-graph`'s orchestration —
    /// which files get which calls, in what order — not for this crate's own dispatch.
    fn role(&self) -> ManifestRole;

    fn package_name(&self) -> Result<String, ManifestError>;
    fn current_version(&self) -> Result<Version, ManifestError>;
    fn write_version(&mut self, v: &Version) -> Result<(), ManifestError>;
    fn iter_dependencies(&self) -> Box<dyn Iterator<Item = DependencyEntry> + '_>;
    fn update_dependency_spec(&mut self, name: &str, kind: DepKind, new: DepSpec)
        -> Result<(), ManifestError>;
    fn update_optional_dependencies(&mut self, updates: &[(String, Version)])
        -> Result<(), ManifestError>;
}
```

#### CM.1.1 Persistence model

> `[SPEC DECISION, not in 00-design.md: every mutating method persists to disk before
> returning `Ok`.]` §15's trait has no `flush`/`save`/`write_to_disk` method, and §7.6's
> mutation phase calls multiple mutating methods on the same manifest across steps 3/4/5/6 (a
> napi main package gets `write_version` at step 3, `update_optional_dependencies` at step 5,
> and possibly `update_dependency_spec` at step 6 for its own regular deps) — so either every
> call persists immediately, or the trait needs a method it does not have. The former is the
> smaller change and is what every impl below does: each concrete `Manifest` holds an
> in-memory, format-preserving document (`toml_edit::DocumentMut` for `CargoToml`, the
> order-preserving tree described in §CM.5 for `PackageJson`) and re-serializes + `fs::write`s
> the whole file at the end of each mutating call. This costs a few redundant small-file
> rewrites per package per run — napi-scale workspaces have tens, not thousands, of manifests
> — in exchange for a trait surface that matches §15 exactly and a persistence story with no
> separate commit step to forget (P3: every mutating call is independently idempotent and
> independently safe to be the last one that runs before a crash).

A read-only method (`path`, `ecosystem`, `role`, `package_name`, `current_version`,
`iter_dependencies`) never touches disk after `open()` — everything it needs was loaded into
the in-memory document at construction time.

#### CM.1.2 Error-variant coverage

Every `ManifestError` variant this crate's impls can produce, so the mapping is complete
and traceable rather than left to be discovered per call site:

| Method | Variants it can return |
|---|---|
| `open()` (factory, §CM.2) | `Read`, `Parse`, `AbsolutePath`/`NonUtf8Path` (via `callisto-model`'s path validation), `ReadOnlyFormat` (lockfile role, §CM.2.4; unimplemented format, §CM.7), `InvalidRoleForFormat` (via `callisto-model`) |
| `package_name` | `MissingField` |
| `current_version` | `MissingField`, `InvalidVersion`, `WorkspaceInherited` (only if `open()` was called with no resolved inheritance context, §CM.4.4) |
| `write_version` | `Write`, `WorkspaceInherited`, `FormattingNotPreserved` |
| `iter_dependencies` | none — infallible; unparseable entries surface as `DepSpec::Opaque`, never as an iteration error (§CM.3) |
| `update_dependency_spec` | `DependencyNotFound`, `WorkspaceInherited`, `Write`, `FormattingNotPreserved` |
| `update_optional_dependencies` | `UnsupportedOperation` (`CargoToml`, unconditionally), `Write`, `FormattingNotPreserved` |

### CM.2 Opening manifests — `OpenContext` and the `open()` factory

The trait's methods all take `&self`/`&mut self` with no extra parameters, so anything a
format needs *beyond its own file* to answer a read correctly (Cargo workspace inheritance,
§CM.4.4; which tool governs `workspace:` protocol strings, §CM.5.4) has to be resolved once,
up front, and carried into the concrete type at construction time — not threaded through
every call.

```rust
/// Everything a concrete `Manifest::open` needs beyond the manifest's own file. Resolved
/// once per callisto invocation (`version`, `status`, `plan-publish`, …) and shared across
/// every `open()` call in that invocation — never re-derived per package (P2/P3: cheap,
/// idempotent, no redundant I/O; re-deriving per package would also risk two packages
/// opened in the same run disagreeing about, e.g., which lockfile governs `workspace:`
/// resolution, which is a workspace-wide fact, not a per-package one).
pub struct OpenContext<'a> {
    /// The workspace root every `ManifestDecl.path` is relative to (§M.1.3). This is the one
    /// absolute path in the system; every path stored in a value type stays relative to it.
    pub workspace_root: &'a Path,

    /// `Some` iff the workspace contains at least one Cargo.toml and its root declares
    /// `[workspace.package]` and/or `[workspace.dependencies]` (§18 Q2, §CM.4.4). `None` is
    /// the routine case for a workspace with no Cargo inheritance in use at all — not an
    /// error, since inheritance is opt-in Cargo syntax, not a Cargo-workspace requirement.
    pub cargo_workspace: Option<Arc<WorkspaceInheritance>>,

    /// Which npm-ecosystem tool's `workspace:` protocol convention governs this workspace
    /// (§CM.5.4). `None` when there is no npm workspace at all (a lone `package.json` with
    /// no `"workspaces"` key) — in that case no dependency string in it can legally use
    /// `workspace:` syntax, so `Workspace`-variant parsing is simply never attempted.
    pub npm_workspace_kind: Option<WorkspaceKind>,
}

/// Constructs the concrete `Manifest` impl for `decl`, dispatching on `decl.format`.
/// Feature-gated per format: **a format whose feature is disabled does not appear in this
/// function's `match` at all**, so passing one is a compile-time impossibility for a caller
/// that only enabled `cargo`, not a runtime error path a fixture has to cover. §11's wasm
/// build line (`--features "wasm,cargo,npm"`) is what makes this concrete: the
/// `pypi`/`go`/`maven`/… arms plain do not exist in that build.
pub fn open(decl: &ManifestDecl, ctx: &OpenContext<'_>)
    -> Result<Box<dyn Manifest>, ManifestError>
{
    if decl.role == ManifestRole::Lockfile {
        return Err(ManifestError::ReadOnlyFormat {
            path: decl.path.clone(),
            format: decl.format,
            reason: "lockfiles are regenerated via subprocess (§7.6 step 9), never opened \
                     as a Manifest handle",
        });
    }
    match decl.format {
        #[cfg(feature = "cargo")]
        ManifestFormat::CargoToml => Ok(Box::new(cargo::CargoToml::open(decl, ctx)?)),
        #[cfg(feature = "npm")]
        ManifestFormat::PackageJson => Ok(Box::new(npm::PackageJson::open(decl, ctx)?)),
        other => Err(ManifestError::ReadOnlyFormat {
            path: decl.path.clone(),
            format: other,
            reason: "not implemented — demand-gated per §2.2, see §CM.7",
        }),
    }
}
```

> `[SPEC DECISION, not in 00-design.md: `OpenContext`'s exact shape and the `open()` factory
> signature.]` §15 gives the `Manifest` trait but not a constructor — someone has to turn a
> `ManifestDecl` into a `Box<dyn Manifest>`, and §18 Q2's `WorkspaceCargoResolver` has to
> reach every `Cargo.toml` handle *somehow*. This shape is the smallest one consistent with
> P4 (uniform ecosystem behind traits — one factory, not one per format that `callisto-graph`
> has to know how to call) and P2 (resolve workspace-wide facts once, not per package).

#### CM.2.4 Why `ManifestRole::Lockfile` is refused, not silently ignored

§5.2: *"Lockfile — regenerated, not directly version-written."* §7.6 step 9 regenerates
lockfiles via `--refresh-lockfiles`, which runs a subprocess (`cargo generate-lockfile`,
`pnpm install --lockfile-only`, …) through `CommandRunner` — not through this trait.
`CargoLock`/`PackageLockJson`/`PnpmLockYaml`/`YarnLock` are declared `ManifestFormat` variants
(§5.2, §M.6.2) precisely so a `Package`'s `manifests: Vec<ManifestDecl>` can *name* its
lockfiles for `--refresh-lockfiles` bookkeeping (§M.12.3's `LockfileRefreshResult.filename`),
without this crate ever being asked to open one as a `Manifest` object — every one of the
trait's methods (`current_version`, `write_version`, …) is meaningless for a lockfile.
Refusing at `open()` turns a caller mistake into an immediate, specific error instead of a
confusing downstream failure inside `iter_dependencies` or similar.

### CM.3 `DepSpec` parsing and round-trip — shared vocabulary

Two independent operations per manifest format, both format-specific because the *string
grammar* of a version requirement differs by ecosystem (Cargo's comma-AND clauses vs. npm's
hyphen ranges and `||` OR-groups are not the same language, even though both ultimately
constrain a SemVer version):

1. **Parse** — raw spec string → `DepSpec`, performed once per entry, inside
   `iter_dependencies()`. Infallible: an unrecognized string becomes `DepSpec::Opaque` rather
   than an error (§7.3: *"anything unrecognized — left untouched"*).
2. **Round-trip rewrite** — an existing `DepSpec` plus a target `Version` → `Option<DepSpec>`.
   `None` means "no confident rewrite exists."

```rust
/// Dispatches round-trip rewriting to the ecosystem-specific grammar. Exposed as one
/// function so `callisto-graph`'s cascade step does not need a `match` on `Ecosystem` of its
/// own — that `match` belongs here, next to the grammars it dispatches between (P4).
///
/// Deliberately operates on the **preserved original string** inside `Range`/`Exact`/
/// `CargoBare`, not on a re-derived `VersionReq`: textually patching just the version
/// substring inside an already-validated pattern is what makes single-clause rewrites exact
/// and multi-clause rewrites a confident `None` rather than a best-effort guess (§7.3:
/// *"complex multi-clause ranges… fall back to Opaque and warn"*).
///
/// **This is the grammar half only.** The *policy* half — whether to attempt a rewrite at
/// all (`cascade.preserve-npm-ranges`), verifying the candidate re-parses and still covers,
/// and turning `None` into `DiagnosticCode::RangeNotRoundTrippable` — lives in
/// `callisto-graph::rewrite_spec` (§G.7.7). Splitting it this way keeps ecosystem grammar
/// knowledge out of the graph and cascade policy out of the manifests crate; §11 records the
/// reconciliation, since two drafts had each described the whole operation as theirs.
pub fn round_trip(ecosystem: Ecosystem, spec: &DepSpec, target: &Version) -> Option<DepSpec> {
    match (ecosystem, spec) {
        #[cfg(feature = "cargo")]
        (Ecosystem::Cargo, _) => cargo::round_trip(spec, target),
        #[cfg(feature = "npm")]
        (Ecosystem::Npm, _) => npm::round_trip(spec, target),
        _ => None,
    }
}
```

**Never call `update_dependency_spec` with an `Opaque` target.** An untouched spec requires no
write at all — calling it anyway would be a needless write for an unchanged value at best,
and at worst reformats surrounding TOML/JSON the original did not need reformatted. The
caller (`callisto-graph`) simply omits the call when `rewrite_spec` yields `LeftAlone`.

**Never call it with `Workspace` or `Catalog` either.** `Workspace` because pnpm/yarn resolve
it locally regardless of version (§7.3: *"never bumped"*); `Catalog` because callisto never
rewrites catalog entries at all (§M.7.2, `DiagnosticCode::CatalogSpecNotRewritten`).
`round_trip` returns `None` for both, as a structural guarantee rather than something the
caller has to remember to skip.

### CM.4 The `cargo` feature — `CargoToml`

#### CM.4.1 Storage and format preservation

```rust
pub struct CargoToml {
    path: PathBuf,                                  // workspace-root-relative (§M.1.3)
    absolute: PathBuf,                              // resolved against ctx.workspace_root
    role: ManifestRole,
    document: toml_edit::DocumentMut,               // format-preserving in-memory tree
    inherited_deps: HashSet<(DepKind, String)>,     // names whose spec is `foo.workspace = true`
    inherited_version: bool,                        // `version.workspace = true` at [package]
    inheritance: Option<Arc<WorkspaceInheritance>>, // resolved values, for reads only
}
```

Format preservation is `toml_edit`'s job (§7.6 step 3: *"format-preserving: `toml_edit` for
TOML"*) — every mutating method below edits the existing `Item` in place
(`document["dependencies"]["foo"]["version"] = value(new_str)`, never
`document["dependencies"]["foo"] = new_table`), which is what makes `toml_edit` preserve
comments, blank lines, key order, and inline-table formatting the caller did not touch.
`ManifestError::FormattingNotPreserved` is reserved for the case `toml_edit` itself reports a
non-representable edit (extremely rare — e.g. a value that cannot round-trip as a TOML string
literal); it is not expected to fire in ordinary operation and exists so a fixture failure
here is diagnosable rather than a generic IO error (§M.13.2).

#### CM.4.2 `DepSpec` parse dispatch

For each entry under `[dependencies]`, `[dev-dependencies]`, `[build-dependencies]`, and
their `[target.'cfg(…)'.*]` variants (walked but not otherwise treated specially — a
target-gated dependency is still a real edge; §7.2 says nothing exempts them):

| Raw TOML shape | `DepKind` | `DepSpec` | Notes |
|---|---|---|---|
| `foo = { workspace = true, … }` | per section | *(resolved via `inheritance`, §CM.4.4)* | Name recorded in `inherited_deps`; the *resolved* `DepSpec` (from the root's `[workspace.dependencies]`) is what `iter_dependencies` yields, not a placeholder — a caller should never have to know inheritance was involved to read the effective spec. The yielded `DependencyEntry` carries `inherited: true` (§M.7.3) and the **member's own section kind**, which is the pair §G.4.4 needs to route the rewrite to the root without losing the edge's kind. Every other row yields `inherited: false`. |
| `foo = "1.2.3"` (bare, full `X.Y.Z[-pre][+build]`, no operator/comma/wildcard) | per section | `CargoBare(Version)` | The full string parses cleanly as a `Version` under `VersionGrammar::SemVer` with nothing left over. |
| `foo = "1.2"`, `"1"`, `"^1.2.3"`, `"~1.2"`, `">=1.0, <2.0"`, `"*"`, `"=1.2.3"` | per section | `Range(VersionReq, original)` | Anything with an operator, a comma-joined clause, a wildcard, or a partial (non-3-component) bare version — Cargo's own semver-req grammar handles all of these; `CargoBare` is reserved for the no-operator, no-ambiguity case specifically so a round-trip write does not introduce an operator that was not there (§CM.4.3). |
| `foo = "1.2.3"` under `[dependencies]` with a sibling `optional = true` in the same inline/dotted table | `Optional` | as above | Cargo has no separate optional-dependency *section* (unlike npm) — `optional = true` is a flag on a normal `[dependencies]` entry. This is the one place Cargo's `DepKind` tagging needs a second field read, not just a section name. |
| `foo = { git = "…", branch = "…" }`, `foo = { path = "../foo" }`, any table with no plain `version` string this parser recognizes | per section | `Opaque(rendered)` | `rendered` is `toml_edit::Item::to_string()`'s own output for that entry, trimmed — the *original* formatting, kept for diagnostics only. Never a write target (§CM.3). |

`DepKind::Peer` never appears for Cargo — Rust has no manifest concept of a peer dependency.
A caller invoking `update_dependency_spec(name, DepKind::Peer, _)` on a `CargoToml` gets
`ManifestError::DependencyNotFound { path, name, kind: Peer }` — there being no
`[peer-dependencies]` section at all is represented the same way as "no entry with that name
in the section that does exist," since both mean "nothing here to update."

#### CM.4.3 `cargo::round_trip`

```rust
/// §7.3, §13 inv. 15. Textual, single-clause only — recognizes the operator prefix
/// (none, `^`, `~`, `>=`, `>`, `<=`, `<`, `=`) at the front of the preserved original
/// string, replaces only the version substring that follows it, and refuses (returns
/// `None`) for anything with a comma, `*`, or more than one recognizable clause. A
/// comma-joined requirement like `">=1.2, <2.0"` genuinely has two independent bounds; a
/// mechanical "replace the version" has no single correct answer for which bound to move,
/// so it is exactly the case §7.3 names as falling back to `Opaque`.
///
/// **Precision is preserved**: `^1.2` → `^1.3`, never `^1.3.0`. A precision change is a diff
/// a reviewer has to read for no gain.
fn round_trip(spec: &DepSpec, target: &Version) -> Option<DepSpec> {
    match spec {
        DepSpec::CargoBare(_) => Some(DepSpec::CargoBare(target.clone())),
        DepSpec::Range(_, original) => {
            let (prefix, rest) = split_single_operator_prefix(original)?;
            if rest.contains(',') || rest.contains('*') {
                return None; // multi-clause or wildcard — not confidently rewritable
            }
            let rendered = format!("{prefix}{}", render_at_precision(target, rest));
            let req = VersionReq::parse(&rendered, Ecosystem::Cargo).ok()?;
            Some(DepSpec::Range(req, rendered))
        }
        _ => None,
    }
}

/// Renders `target` with the same number of dot-separated components the original clause
/// used: `("1.2", 1.3.0)` → `"1.3"`, `("1.2.3", 1.3.0)` → `"1.3.0"`, `("1", 2.0.0)` → `"2"`.
fn render_at_precision(target: &Version, original_clause: &str) -> String;
```

#### CM.4.4 `WorkspaceCargoResolver` and `WorkspaceInheritance`

> `[SPEC DECISION, not in 00-design.md: the concrete shape below.]` §18 Q2 commits to
> `WorkspaceCargoResolver` existing, in `callisto-manifests`, v0.1, resolving
> `[workspace.dependencies]` inheritance — but gives no signature. The split into a read-only
> `WorkspaceInheritance` snapshot (cheap to `Arc`-share into every member `CargoToml::open()`
> call) and a separately-owned, mutable `WorkspaceCargoResolver` (the only thing that ever
> writes the root) is the smallest shape that lets many member handles read inherited values
> concurrently without each holding a mutable reference to the same root document — Rust's
> aliasing rules would forbid that outright, and a `Mutex`/`RefCell` would be paying runtime
> cost for a document that, for reads, never changes after it is loaded once per invocation
> (P2/P3).

```rust
/// A read-only snapshot of the workspace root's inheritable Cargo values, resolved once per
/// invocation (§CM.2) and shared by every member `CargoToml::open()` call via
/// `OpenContext.cargo_workspace`.
#[derive(Clone, Debug, Default)]
pub struct WorkspaceInheritance {
    /// The workspace root's own `Cargo.toml`, workspace-root-relative (§M.1.3) — the file
    /// that *declares* every value below, and therefore the write target for all of them
    /// (§18 Q2: "bumping a member's dep means editing the root, not the member").
    pub root_manifest: PathBuf,
    /// `[workspace.package].version`, when present.
    pub version: Option<Version>,
    /// `[workspace.dependencies].*`, parsed with the identical dispatch as §CM.4.2 (a
    /// workspace-level dependency entry uses the same TOML shapes a member's does).
    /// **Keyed by name only** — the root's table has no sections, so it carries no `DepKind`.
    pub dependencies: BTreeMap<String, DepSpec>,
}

impl WorkspaceInheritance {
    /// The read accessor graph construction uses when a member's `DependencyEntry` carries
    /// `inherited: true` (§M.7.3, §G.4.4). `None` when the root declares no
    /// `[workspace.dependencies]` entry of that name — which, for an entry that *said*
    /// `foo.workspace = true`, is a malformed workspace and is raised as
    /// `ManifestError::MissingField { path: root_manifest, field: "workspace.dependencies" }`
    /// by the caller rather than silently dropping the edge.
    ///
    /// **Kind-preserving by omission, deliberately.** This returns no `DepKind`, because the
    /// root's `[workspace.dependencies]` table has none to give. The caller keeps the
    /// *member's* declaring-section kind (`DependencyEntry::kind`), so a dev-dependency
    /// inherited from the root still produces a `Dev`-kind edge and still takes §7.4's `Dev`
    /// row (`Severity::None`, spec rewrite only) — a resolution that returned the root's
    /// "kind" would have to invent one, and inventing `Runtime` would silently promote every
    /// inherited dev-dependency into a version-bumping edge.
    pub fn inherited(&self, name: &str) -> Option<InheritedDep<'_>>;
}

/// One resolved `[workspace.dependencies]` entry, plus the file that declares it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InheritedDep<'a> {
    pub spec: &'a DepSpec,
    /// Always `WorkspaceInheritance::root_manifest`. Carried on the result rather than
    /// re-read from the snapshot at the call site so that `DepEdge::from_manifest` and
    /// `DepWriteTarget::CargoWorkspaceDependency` (§G.7.3) are populated from one value.
    pub declared_in: &'a Path,
}

/// The mutable editor for the workspace root's `Cargo.toml` — the only type in this crate
/// that writes `[workspace.package]`/`[workspace.dependencies]`. A member `CargoToml` whose
/// `write_version`/`update_dependency_spec` is called on an inherited field refuses
/// (`ManifestError::WorkspaceInherited`, §CM.1.2) specifically so the caller is forced
/// through this type instead of writing a member-local override that would silently shadow
/// the workspace value — exactly the failure mode `WorkspaceInherited`'s own doc comment
/// names (§M.13.2).
pub struct WorkspaceCargoResolver {
    root_path: PathBuf,
    document: toml_edit::DocumentMut,
}

impl WorkspaceCargoResolver {
    /// Reads and parses the workspace root's `Cargo.toml`. A root with no
    /// `[workspace.package]` and no `[workspace.dependencies]` table is valid —
    /// `inheritance()` then returns `WorkspaceInheritance::default()`, meaning no member can
    /// legally use `.workspace = true` (a member that does anyway is a Cargo-level error, not
    /// this resolver's to catch — `cargo metadata`/`cargo check` will refuse it before
    /// callisto ever runs).
    pub fn load(root_manifest_path: &Path) -> Result<Self, ManifestError>;

    /// Cheap: extracts and parses the current `[workspace.package].version`/
    /// `[workspace.dependencies]` into an owned snapshot. Called once per invocation, then
    /// wrapped in `Arc` for `OpenContext` (§CM.2).
    pub fn inheritance(&self) -> Result<WorkspaceInheritance, ManifestError>;

    /// Writes `[workspace.package].version`. The correct target for *any* member whose
    /// `write_version` refused with `WorkspaceInherited { key: "version", .. }` — since
    /// `version.workspace = true` is (in practice) used by every member that opts in, one
    /// call here typically satisfies every such member for the run, which is the entire point
    /// of the Cargo feature this resolves (§18 Q2).
    pub fn write_version(&mut self, v: &Version) -> Result<(), ManifestError>;

    /// Writes one `[workspace.dependencies]` entry. The apply-phase target for every
    /// `SpecRewrite` whose `DepWriteTarget` is `CargoWorkspaceDependency` (§G.7.3, §G.10.2
    /// step 6), and the correct target for a member's `update_dependency_spec` refusal with
    /// `WorkspaceInherited { key: name, .. }` — rewriting it here is correct Cargo semantics
    /// (§18 Q2: *"Bumping a member's dep means editing the root, not the member"*), and
    /// affects every member that inherits `name`, which is the intended effect, not a side
    /// effect to guard against.
    ///
    /// **Takes no `DepKind`**, for the same reason `WorkspaceInheritance::inherited` returns
    /// none: the root's table has no sections. One call updates the entry every inheriting
    /// member sees, whatever section each of them declared it under.
    pub fn write_dependency(&mut self, name: &str, new: DepSpec) -> Result<(), ManifestError>;
}
```

**Reads are transparent; writes are refused and redirected.** A member `CargoToml` opened
with `ctx.cargo_workspace = Some(inheritance)` answers `current_version()` and
`iter_dependencies()` with the *real, resolved* values — a caller reading a member's version
or dependency list never needs to know inheritance was involved. Only a *write* attempt
directly on the member surfaces `WorkspaceInherited`, because that is the one operation where
"which file actually gets edited" matters and getting it wrong (writing a local override) is a
silent correctness bug, not merely redundant work.

If `open()` is called for a decl that uses `.workspace = true` but `ctx.cargo_workspace` is
`None` (no resolver context was ever built for this invocation), `open()` itself fails with
`ManifestError::Read { path, message: "declares .workspace = true but no
WorkspaceCargoResolver context was supplied" }` — a caller-contract violation caught at
construction rather than deferred to the first read. `callisto-graph` always builds a
`WorkspaceCargoResolver` once per invocation whenever any `Cargo.toml` is present in the
workspace (§G.4.1), matching §18 Q2's *"must be handled correctly from v0.1."*

### CM.5 The `npm` feature — `PackageJson`

#### CM.5.1 The format-preserving editor — what "custom" has to preserve

JSON has no `toml_edit` equivalent to reach for, because JSON (unlike TOML) has no comments
and no whitespace attached to a specific node a library could hang a round-trip guarantee on
— there is nothing *in* a parsed JSON tree that remembers how it was indented. What
"format-preserving" can mean for JSON is therefore narrower and fully enumerable, and npm's
own tooling (`detect-indent` + `write-file-atomic`) solves it the same way callisto does here:
**detect a small, fixed set of format degrees of freedom once, on read, and reproduce them
exactly on write.** There is nothing else to preserve — `package.json` has no multi-line
strings, no significant blank lines, no trailing commas.

```rust
pub struct PackageJson {
    path: PathBuf,
    absolute: PathBuf,
    role: ManifestRole,
    /// `serde_json::Map<String, serde_json::Value>`, order-preserving. Requires this
    /// crate's `Cargo.toml` to enable `serde_json`'s `preserve_order` feature.
    doc: serde_json::Map<String, serde_json::Value>,
    fingerprint: FormatFingerprint,
    npm_workspace_kind: Option<WorkspaceKind>,
}

/// The full, closed set of format facts this editor reproduces. Nothing beyond these three
/// varies between two semantically-identical `package.json` files in the wild.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FormatFingerprint {
    indent: Indent,
    trailing_newline: bool,
    line_ending: LineEnding,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Indent {
    Spaces(u8),
    Tabs,
    /// No indented line existed to sample (e.g. `{}`, or already-minified JSON). §6.4
    /// already commits callisto to 2-space JSON elsewhere (`pre.json`); reusing that default
    /// here rather than inventing a second one is the smaller decision.
    DefaultTwoSpaces,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LineEnding { Lf, CrLf }
```

> `[SPEC DECISION, not in 00-design.md: the exact fingerprint fields and the 2-space
> fallback.]` §6.4 pins 2-space JSON for `pre.json` specifically; extending that same fallback
> to a `package.json` whose indentation cannot be detected (rather than inventing an unrelated
> default) is the smallest consistent choice. Detecting and preserving CRLF is not mentioned
> anywhere in 00-design.md; it is included because a Windows-authored `package.json` committed
> with CRLF is common enough that silently normalizing it to LF on every callisto write would
> be a gratuitous diff-noise regression relative to `@changesets/cli` itself, which preserves
> it. Note the deliberate contrast with §F.5.3, where changeset files *are* normalized: a
> `package.json` is a user-owned file callisto edits in place; a `.changeset/*.md` is a file
> callisto authors.

**Read algorithm:**
1. Read the file, decode UTF-8 (`ManifestError::Read` on failure — not `NonUtf8Path`, which is
   reserved for *path* UTF-8, §M.13.1).
2. Scan for the first `\r\n` vs. bare `\n` to fix `line_ending`.
3. Check whether the raw text ends with that same line-ending sequence → `trailing_newline`.
4. Scan for the first line beginning with a run of whitespace immediately followed by `"` (a
   nested object key) to fix `indent` — a tab in that run means `Indent::Tabs`; otherwise
   count the spaces. No such line (flat or empty object) → `Indent::DefaultTwoSpaces`.
5. Parse with `serde_json::from_str::<serde_json::Map<_, _>>` → `ManifestError::Parse` on
   failure.

**Write algorithm** (every mutating method ends by calling this, per §CM.1.1):
1. Serialize `doc` via `serde_json::Serializer` with `PrettyFormatter::with_indent`
   configured from `fingerprint.indent` (`b"  "` / `b"    "` / `b"\t"` as appropriate).
2. If `fingerprint.line_ending == CrLf`, replace every `\n` the serializer produced with
   `\r\n`.
3. Append the line-ending sequence iff `fingerprint.trailing_newline`; otherwise leave the
   output exactly as serialized (no trailing newline).
4. `fs::write` the result to `self.absolute`. `ManifestError::Write` on failure.

**Key-order mutation semantics**, which is the other half of "custom" beyond raw bytes:
`serde_json::Map` under `preserve_order` is backed by an `IndexMap`, whose `insert` on an
**existing** key updates the value without moving its position — so `write_version` and an
`update_dependency_spec` targeting an already-present key never reorder anything, which is
the common case and the one that matters for minimizing diff noise. A **new** key (a napi
platform package added to `optionalDependencies` for the first time, §CM.6) is appended at the
end of its containing object, matching the position `npm`/`napi-rs` itself would use when
scaffolding — there is no existing key to preserve a position relative to, so "append" is the
only position that does not invent a preference the original file never expressed.

> `[SPEC DECISION, not in 00-design.md: enabling `serde_json/preserve_order` is a
> whole-workspace Cargo feature unification concern, not one scoped to this crate.]` Cargo
> features are additive across the dependency graph, so any crate anywhere in the workspace
> that also depends on `serde_json` picks up order preservation too the moment this crate
> enables it — `callisto-model`'s report serialization and `callisto-format`'s `pre.json`
> writer both do. This is called out explicitly because it is a genuine, easy-to-miss Cargo
> gotcha, not because it is harmful: order preservation is a strict superset of correctness
> for every other consumer (nothing anywhere in this design relies on `serde_json::Map` being
> sorted, and §F.7 actively wants insertion order), so the unification is free, just
> non-obvious.

#### CM.5.2 `DepSpec` parse dispatch

For each entry under `dependencies`, `devDependencies`, `peerDependencies`,
`optionalDependencies`:

| Raw string | `DepKind` (from section) | `DepSpec` | Notes |
|---|---|---|---|
| `"1.2.3"` (bare, full `X.Y.Z[-pre][+build]`) | per section | `Exact(Version)` | npm treats a bare version as an exact-match requirement, **not** a caret default — the opposite of Cargo's bare-version convention. This is exactly why `DepSpec` has two separate bare-shaped variants (`Exact` vs. `CargoBare`) instead of one: the same literal string means a different requirement in each ecosystem. |
| `"workspace:*"`, `"workspace:^"`, `"workspace:~"`, `"workspace:1.2.3"` | per section | `Workspace(kind)` | `kind` is `ctx.npm_workspace_kind` (§CM.5.4), not derived from the string — pnpm and Yarn Berry share identical `workspace:` protocol syntax, so the string alone cannot distinguish them; which tool governs is a workspace-wide fact resolved once. |
| `"catalog:"`, `"catalog:reactBundle"` | per section | `Catalog(None \| Some(name))` | pnpm catalog reference. `Coverage::Unknown`, never rewritten (§M.7.2). |
| `"^1.2.3"`, `"~1.2.3"`, `">=1.2.3 <2.0.0"`, `"1.2.3 - 2.3.4"`, `"1.x"`, `"*"`, `"1.2.3 \|\| 2.0.0"` | per section | `Range(VersionReq, original)` | npm's fuller range grammar (hyphen ranges, `x`/`X` partial wildcards, `\|\|` OR-groups, space-separated AND) — all handled by the underlying `VersionReq` parse; `*` alone parses here too (wildcard-any), distinct from the workspace/catalog protocol strings above. |
| `git+https://…`, `file:../foo`, `link:../foo`, `npm:@scope/real-name@^1.0.0` (aliasing) | per section | `Opaque(original)` | Left untouched byte-for-byte; never a round-trip target. |

`DepKind::Build` never appears for npm — there is no `buildDependencies` section in
`package.json`. `update_dependency_spec(name, DepKind::Build, _)` on a `PackageJson` returns
`ManifestError::DependencyNotFound { path, name, kind: Build }`, the same "no such section,
therefore no such entry" treatment as Cargo's missing `Peer` section (§CM.4.2) — one rule,
applied symmetrically in both directions, rather than two format-specific special cases.

#### CM.5.3 `npm::round_trip`

Same textual, single-clause-only shape as `cargo::round_trip` (§CM.4.3), with npm's own
operator vocabulary (`^`, `~`, `>=`, `>`, `<=`, `<`, `=`, bare-as-exact) and npm's own refusal
set: any hyphen range, `x`/`X` wildcard, `||` OR-group, or space-separated multi-clause range
returns `None` rather than a guessed rewrite — those are exactly the "complex multi-clause
ranges" §7.3 names.

```rust
fn round_trip(spec: &DepSpec, target: &Version) -> Option<DepSpec> {
    match spec {
        DepSpec::Exact(_) => Some(DepSpec::Exact(target.clone())),
        DepSpec::Range(_, original) => {
            let (prefix, rest) = split_single_operator_prefix(original)?;
            if rest.contains(' ') || rest.contains('-') || rest.contains('|')
                || rest.contains(['x', 'X', '*'])
            {
                return None;
            }
            let rendered = format!("{prefix}{}", render_at_precision(target, rest));
            let req = VersionReq::parse(&rendered, Ecosystem::Npm).ok()?;
            Some(DepSpec::Range(req, rendered))
        }
        _ => None, // Workspace, Catalog, Opaque, CargoBare — never rewritten (§CM.3)
    }
}
```

#### CM.5.4 Detecting `WorkspaceKind`

```rust
/// Resolved once per invocation from lockfile presence at the workspace root (§CM.2), never
/// per package — which lockfile is on disk is a workspace-wide fact.
pub fn detect_npm_workspace_kind(workspace_root: &Path)
    -> Result<Option<WorkspaceKind>, ManifestError>
{
    // pnpm-lock.yaml present                                 -> Some(Pnpm)
    // yarn.lock present                                      -> Some(Yarn)
    // package-lock.json present, or root package.json has a
    //   "workspaces" key and neither lockfile above exists    -> Some(Npm)
    // no lockfile and no "workspaces" key anywhere            -> None
}
```

> `[SPEC DECISION, not in 00-design.md: this detection algorithm.]` 00-design.md establishes
> that `WorkspaceKind` exists and is `DepSpec::Workspace`'s payload (§5.1) but never says how
> it is derived. Lockfile presence is the only unambiguous, syntax-free signal available —
> `package.json`'s own `"workspaces"` key is shared verbatim by npm and Yarn classic, so it
> alone cannot distinguish them, and only the lockfile choice actually determines which
> `workspace:` protocol dialect (if any) the tooling honours.

### CM.6 Platform manifests and `optionalDependencies` rewriting (§7.6 steps 4–5)

**Step 4 — platform manifests inherit the parent version.** No special-cased code path exists
for this: `callisto-graph` (which alone knows fixed-group membership, §7.5) simply calls
`write_version(&new_version)` on each `ManifestRole::Platform` manifest's handle, identically
to any `Canonical` one. `role()` is what *routes* the call at the orchestration layer; nothing
inside `CargoToml`'s or `PackageJson`'s `write_version` branches on it. A platform
`package.json`'s `os`/`cpu` napi-convention fields are untouched — this crate does not model
or need to know about them.

**Step 5 — `optionalDependencies` pinned to exact platform versions.**
`update_optional_dependencies` is what implements this, called once on the napi *main*
package's handle:

- `PackageJson`: for each `(name, version)` in `updates`, upsert into the top-level
  `optionalDependencies` object as an **exact** string (`version.render()`, no operator
  prefix — napi's own convention pins platform deps exactly, since a platform package's
  version always exactly tracks its main package's). A name absent from `updates` but present
  in `optionalDependencies` is left untouched (§13 inv. 21 applied to this call specifically:
  membership changes are only ever written via `init`'s reviewable flow, never auto-mutated
  here — this method only ever *adds or updates* the names it is given, never removes). If the
  `optionalDependencies` key does not exist yet at all, it is created — appended at the end of
  the top-level object per §CM.5.1's key-order rule.
- `CargoToml`: **always** `Err(ManifestError::UnsupportedOperation { path, format: CargoToml,
  operation: "update_optional_dependencies" })` — this is the literal example
  `ManifestError::UnsupportedOperation`'s own doc comment cites (§M.13.2). Crates have no
  npm-`optionalDependencies`-shaped concept; a crate that is also separately published to
  crates.io (§9.2's `rustCrates[]`) has nothing here to pin.

### CM.7 Ecosystem write-target conventions (§7.8)

Committed, implemented at v0.1 — Rust and npm:

| Ecosystem | Write target | Notes |
|---|---|---|
| Cargo | `Cargo.toml` (member and/or root, §CM.4.4) | `Cargo.lock` is `ManifestRole::Lockfile`, never opened as a `Manifest` (§CM.2.4). |
| npm | `package.json` | `package-lock.json`/`pnpm-lock.yaml`/`yarn.lock` are `ManifestRole::Lockfile`, same treatment. |

Demand-gated, **not implemented** — declared only as `ManifestFormat` variants already
present in `callisto-model` (§M.6.2) so the enum stays open per P4, with no behaviour built
behind them, per §2.2's own scope discipline. `open()` returns
`ReadOnlyFormat { reason: "not implemented — demand-gated per §2.2" }` for every one of these
until a real user need promotes it:

| Ecosystem | Write target when built | Read-only / unwritable | §7.8 citation |
|---|---|---|---|
| Python | `pyproject.toml` | `SetupCfg` — hard error (not silent skip) directing the user to migrate | *"hard error if only `setup.py` exists"* |
| JVM | `PomXml`, `GradleVersionCatalog`, `SettingsGradle`, `VersionSbt` | `build.gradle[.kts]`, `build.sbt` — AST-aware editing of Turing-complete scripts is not planned | *"imperative… are not [writable]"* |
| Go | `GoMod`, **downstream `require` lines only** | the module's own version — it has none; the git tag *is* the version (§7.7) | *"no write for the module's own version"* |

The Go row is worth naming explicitly as the one case where, even once built, this crate's
`current_version`/`write_version` cannot have their ordinary meaning for a `Canonical`-role Go
manifest — §7.7 already flags this as *"a real deviation… should be documented as such
whenever Go support ships, not glossed over."* This spec inherits that flag rather than
resolving it, since resolving it is exactly the kind of demand-gated design work §2.2 asks
this document not to do prematurely.

> `[SPEC DECISION, not in 00-design.md: demand-gated `ManifestFormat` variants get a
> declared-but-unimplemented `open()` refusal only — no per-ecosystem behaviour design is
> written here.]` §2.2 is explicit that naming the variants a trait needs to stay open is the
> cheap part and writing per-ecosystem behaviour design for languages nobody asked for is the
> scope creep it cut. This crate holds that line: the table above is the entire artefact.

### CM.8 Feature flags and crate layout

```toml
# callisto-manifests/Cargo.toml (representative)
[features]
default = []
cargo = ["dep:toml_edit"]
npm = ["dep:serde_json"]
# Declared, unimplemented at v0.1 — reserved so a future edit adds behavior behind an
# existing name rather than inventing one, and so §11's wasm build line's shape
# (`--features "wasm,cargo,npm"`) does not need to change when one of these lands (§2.2):
pypi = []
go = []
maven = []
nuget = []
deno = []
jsr = []

[dependencies]
callisto-model = { path = "../callisto-model" }
toml_edit = { version = "…", optional = true }
serde_json = { version = "…", optional = true, features = ["preserve_order"] }

[dev-dependencies]
callisto-fixtures = { path = "../callisto-fixtures", features = ["graph"] }
```

No `wasm` feature of its own — nothing in this crate is conditionally WASM-specific
(`std::fs`, `toml_edit`, and `serde_json` all compile and run identically under
`wasm32-wasip1`, §0.1 rule 2). The `wasm` feature named in §11's build line lives on the
top-level extension crate (§MO.7); `cargo`/`npm` unify down into this crate through ordinary
Cargo feature propagation, same name, different crate.

```
callisto-manifests/
├── src/
│   ├── lib.rs         # `Manifest`, `OpenContext`, `open()`, `round_trip()` — the public
│   │                  #   surface consumed by callisto-graph
│   ├── dep_spec.rs    # round_trip() dispatch, split_single_operator_prefix() and
│   │                  #   render_at_precision() shared helpers
│   ├── cargo/          # feature = "cargo"
│   │   ├── mod.rs
│   │   ├── manifest.rs      # CargoToml
│   │   └── workspace.rs     # WorkspaceCargoResolver, WorkspaceInheritance
│   └── npm/             # feature = "npm"
│       ├── mod.rs
│       ├── manifest.rs      # PackageJson
│       ├── format.rs        # FormatFingerprint detection + reserialization (§CM.5.1)
│       └── workspace.rs     # detect_npm_workspace_kind (§CM.5.4)
```

### CM.9 Fixture obligations

Per §12.6 (*"broader than JSON shape alone… manifest write formatting — round-trip fidelity
against fixture files with unusual existing formatting"*), `callisto-fixtures` must carry, for
this crate specifically:

1. **Round-trip fidelity corpus, per format** — a `Cargo.toml`/`package.json` with unusual
   pre-existing formatting (tabs, 4-space indent, no trailing newline, CRLF line endings,
   comments/blank lines around a Cargo dependency table) put through `write_version` /
   `update_dependency_spec` and diffed byte-for-byte against an expected output that changes
   *only* the touched value.
2. **`DepSpec` parse corpus, per format** — one fixture row per table entry in §CM.4.2 and
   §CM.5.2, including the `CargoBare`-vs-`Range` boundary case (`"1.2.3"` vs `"1.2"` vs
   `"^1.2.3"`) and the `Exact`-vs-`CargoBare` divergence for the identical literal string
   `"1.2.3"` across the two ecosystems — this is the single fact most likely to be silently
   gotten wrong by a future edit, so it gets its own named fixture row, not just
   coverage-by-inclusion in a larger table.
3. **Round-trip rewrite corpus** — single-clause specs that succeed (with the exact expected
   rewritten string, including the precision-preservation cases `^1.2` → `^1.3` and `"1"` →
   `"2"`) and multi-clause specs that correctly return `None` (hyphen ranges, `||`, comma-AND,
   wildcards), per format, per §CM.4.3/§CM.5.3.
4. **Cargo workspace-inheritance corpus** — a root `Cargo.toml` with `[workspace.package]
   .version` and `[workspace.dependencies]`, member `Cargo.toml`s using
   `version.workspace = true` / `foo.workspace = true`, asserting: member reads resolve
   transparently; a direct member write returns `WorkspaceInherited`; the corresponding
   `WorkspaceCargoResolver` write lands in the root file, format-preserved; and `open()`
   fails with the specific `Read` message when the context is missing entirely.
5. **`optionalDependencies` rewrite corpus** — a napi main `package.json` fixture proving: an
   existing platform entry updates in place (no key reorder); a newly-added platform entry is
   appended at the end; an entry present on disk but absent from `updates` is left untouched
   (§13 inv. 21).
6. **Lockfile-role refusal** — one fixture per lockfile `ManifestFormat` asserting `open()`
   returns `ReadOnlyFormat`, not a panic or a silently-empty handle.
7. **`wasm32-wasip1` fixture run** — this crate is named explicitly in §13 inv. 26's core
   set; its fixture suite runs under `wasmtime` with only the workspace root preopened,
   proving every path this crate touches stays inside that root (§M.1.3's discipline made
   testable, not just asserted).

### CM.10 Index of `[SPEC DECISION]` flags

| # | Section | Decision |
|---|---|---|
| 1 | §CM.1.1 | Every mutating `Manifest` method persists to disk before returning, since the trait has no separate flush/save method. |
| 2 | §CM.2 | `OpenContext`/`open()` factory shape — resolves workspace-wide context once per invocation, dispatches on `ManifestFormat` behind per-ecosystem feature gates. |
| 3 | §CM.2.4 | `open()` refuses `ManifestRole::Lockfile` — lockfiles are never opened as `Manifest` handles, only regenerated via subprocess (§7.6 step 9). |
| 4 | §CM.4.4 | `WorkspaceCargoResolver`/`WorkspaceInheritance` concrete shape: read-only shared snapshot vs. separately-owned mutable root editor, plus `WorkspaceInheritance::inherited` as the kind-preserving read accessor graph construction calls. |
| 5 | §CM.5.1 | `PackageJson`'s format-fingerprint fields (indent, trailing newline, line ending) and the 2-space fallback, reusing §6.4's existing precedent. |
| 6 | §CM.5.1 | `serde_json/preserve_order` unifies workspace-wide; harmless but non-obvious, so it is stated rather than discovered. |
| 7 | §CM.5.4 | `WorkspaceKind` detected from lockfile presence at the workspace root, resolved once per invocation. |
| 8 | §CM.7 | Demand-gated `ManifestFormat` variants get declared-but-unimplemented `open()` refusal only, per §2.2's scope discipline. |

---

## 5. `callisto-conventional`

**Purpose.** Given a commit range and a policy, tell the caller what severity Conventional
Commits imply for it — the working half of `ReleaseTrigger::Auto`.

**License:** AGPL-3.0 (§16 — coordination logic, no public-JSON-contract types live here).
**Milestone:** v0.2 (§17 — "previously undeclared in any milestone despite the `Auto` variant
existing in `callisto-model` since v0.1; without this crate `Auto`-trigger packages have a
type but no working inference until v0.2").

### C.0 Dependencies and boundaries

| Edge | Kind | Why |
|---|---|---|
| `callisto-conventional → callisto-model` | normal | `CommandRunner`, `CommandError`, `CommitSha`, `PackageId`, `Severity`, `Version` |
| `callisto-fixtures` | **dev** | §C.9 |

**Deliberately absent:** `callisto-graph`. §M.10's SPEC DECISION placed `CommandRunner` in
`callisto-model` *specifically* so this crate could invoke `git` without otherwise depending
on the graph — this crate never sees a `Package`, a `DependencyResolver`, or a
`ProjectLocator`. Zero moon in the dependency tree (§13 inv. 26); builds for `wasm32-wasip1`
and runs its fixture suite under `wasmtime` with only the workspace root preopened, same as
every other core crate (§0.1 rule 2).

The edge that *does* exist in the other direction is `callisto-graph → callisto-conventional`,
behind `callisto-graph`'s optional `inference` feature (§G.6.4). That is where the
`SeverityInference` adapter lives; nothing in this crate knows the trait exists.

**What this crate is not responsible for:** deciding *which* pathspecs or *which* window
belong to a given package (graph construction's job — §7.2's manifest walk knows a `Package`'s
canonical-manifest directories; this crate is handed a plain `&[PathBuf]`), resolving the last
stable release tag (`last_tag_for`/`select_last_tag`, §M.9.4/§G.9.1), or aggregating a
package's *final* pending severity across changesets, fixed-group unions, and inference (§7.1's
job, in `callisto-graph`, of which one input is this crate's `InferredSeverity`).

### C.1 Module layout

```
callisto-conventional/
├── Cargo.toml          # deps: callisto-model, thiserror. dev-deps: callisto-fixtures.
└── src/
    ├── lib.rs
    ├── commit.rs        # ConventionalCommit, CommitFooter, ParsedCommit, parse_commit
    ├── severity.rs      # raw_severity — no pre-major policy here (§C.4; that lives in
                          # callisto-graph, §G.5.3/§G.6.3)
    ├── window.rs        # InferenceWindow, fetch_commits
    ├── pre_cursor.rs    # pre_cursor_ref_name, resolve_pre_cursor, advance_pre_cursor
    ├── infer.rs         # InferenceInput, InferredSeverity, infer_severity
    └── error.rs         # ConventionalError
```

### C.2 Commit grammar and parsing — `commit.rs`

#### C.2.1 Types

```rust
use callisto_model::CommitSha;

/// One parsed commit that matched the Conventional Commits v1.0.0 header grammar:
/// `<type>[(<scope>)][!]: <description>`, optionally followed by a body and footers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConventionalCommit {
    pub sha: CommitSha,

    /// The raw type token as written, e.g. `"feat"`, `"Feat"`, `"fix"`. Case-preserving —
    /// classification (§C.3) is what applies the case rule, not parsing. Keeping the raw
    /// token lets a caller render a diagnostic that shows the user's own text back to them.
    pub commit_type: String,

    /// Text inside `(…)` between the type and the optional `!`/`:`, if present.
    pub scope: Option<String>,

    /// `true` iff the header carried a `!` immediately before `:`, **or** a footer's token
    /// was the literal uppercase string `BREAKING CHANGE` or `BREAKING-CHANGE` (§13 inv. 11,
    /// §C.2.3). The two signals are ORed into one bool because callisto has no use for
    /// distinguishing "which spelling of breaking" — only whether the commit is breaking.
    pub breaking: bool,

    /// Text after `: ` on the header line.
    pub description: String,

    /// Everything between the header and the footer block, if any, trimmed.
    pub body: Option<String>,

    /// Parsed footers, in document order. Values beyond `breaking` detection are carried
    /// through for possible future changelog use but are not consumed by anything today.
    pub footers: Vec<CommitFooter>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitFooter {
    pub token: String,
    pub value: String,
}

/// A commit's message did not match the Conventional Commits header grammar at all — no
/// error, since most repositories have commits that are not conventional and a
/// version-inference tool that errored on every non-conforming commit in history would be
/// unusable. `raw_severity_of` (§C.3) maps this variant to `Severity::None` uniformly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParsedCommit {
    Conventional(ConventionalCommit),
    NonConventional { sha: CommitSha, subject: String },
}

impl ParsedCommit {
    pub fn sha(&self) -> &CommitSha;
    /// The first line, whatever the classification — used by `callisto-changelog`'s
    /// `ChangeSource::Commit` bullets (§CL.3).
    pub fn subject(&self) -> &str;
}

/// Parses one raw commit message (subject + body, as `git log --format=%B` emits it) into
/// its conventional-or-not classification. Infallible — see `ParsedCommit::NonConventional`.
pub fn parse_commit(sha: CommitSha, message: &str) -> ParsedCommit;
```

#### C.2.2 Header grammar

The first line of `message` is matched against:

```
<type>[(<scope>)][!]: <description>
```

- `<type>` — one or more characters up to the first `(`, `!`, or `:`, non-empty, no internal
  whitespace.
- `(<scope>)` — optional, any text (nested balanced content is *not* required to balance; the
  first `)` on the line closes it).
- `!` — optional, must immediately precede `:` if present.
- `: ` — literal colon-space separator. **A colon with no following space does not match**
  (`fix:foo` is not conventional; `fix: foo` is) — this is the Conventional Commits spec's own
  grammar, not a callisto addition.
- `<description>` — the rest of the line, non-empty.

A header line that does not match this shape in full (missing colon-space, empty type, empty
description) produces `ParsedCommit::NonConventional`. `message` is always evaluated
line-by-line starting from index 0, so a subject with an embedded newline before its colon
(malformed input from a broken commit template) is non-conventional, not an error.

> `[SPEC DECISION, not in 00-design.md: type-token classification (§C.3) is case-sensitive,
> exact-match against the lowercase strings `"feat"`, `"fix"`, `"perf"`, though *parsing*
> captures the type token case-preserving.]` §13 invariant 11 pins case-sensitivity for the
> `BREAKING CHANGE` footer specifically but is silent on the type token itself.
> Case-sensitive matching is the safer default given invariant 11's own precedent:
> case-insensitive type matching risks a false positive on ordinary English prose that
> happens to open with "Fix:" or "Feat:" without conventional-commit intent, and the
> real-world Conventional Commits ecosystem (conventional-changelog, commitlint) treats
> lowercase types as canonical. A commit typed `Feat: …` still parses successfully as
> `ConventionalCommit { commit_type: "Feat", .. }`; it simply classifies as `Severity::None`,
> the same as any other non-matching type.

#### C.2.3 Body and footer split

After the header line, remaining content (if any) is `rest`. Within `rest`, footers are
detected as follows:

1. Scan `rest` line by line for the first line matching **one** of:
   - `` `^([A-Za-z][A-Za-z0-9-]*): (.+)$` `` — a hyphenated-or-alphanumeric token followed by
     `: `.
   - `` `^([A-Za-z][A-Za-z0-9-]*) #(.+)$` `` — the spec's alternate `token #value` form.
   - the two-word literal `` `^BREAKING CHANGE: (.+)$` `` (uppercase only, §13 inv. 11).
2. If no line matches, `rest` in its entirety is `body`, and `footers` is empty.
3. If a line matches at index `i`, everything before it (trimmed) is `body` (`None` if empty),
   and everything from index `i` onward is the footer block: each subsequent line that itself
   matches the footer grammar starts a new `CommitFooter`; a non-matching line is appended
   (with its own leading whitespace trimmed) to the *previous* footer's `value` as a
   continuation.
4. `breaking` is set from parsing (`!` in the header) **or** from any footer whose `token` is
   exactly `"BREAKING CHANGE"` or `"BREAKING-CHANGE"` (both spellings are legal per the
   Conventional Commits spec; both must match uppercase-only, per §13 inv. 11 —
   `"Breaking change:"` and `"breaking-change:"` are not detected, matching real-world tooling
   and the spec's own examples).

> `[SPEC DECISION, not in 00-design.md: footer-block detection uses "the first line matching
> footer grammar starts the footer block; everything before it is body" rather than a fully
> RFC-822-conformant continuation-aware parse.]` Nothing in this design consumes footer
> *values* other than the breaking-change detection in point 4 — no changelog author-credit or
> issue-linking footer is read anywhere in §7 or §9 (and §CL.1 explains why: `callisto-changelog`
> is a single built-in generator with no GitHub-API dependency, so there is nothing for a
> `Closes #123` footer to become) — so a simplified split that correctly isolates
> `BREAKING CHANGE`/`BREAKING-CHANGE` is sufficient. A fully general footer parser would be
> effort spent on a capability nothing downstream uses.

### C.3 Raw severity classification — `severity.rs`

```rust
use callisto_model::Severity;

/// The severity a single commit implies **before** any pre-major remap (§C.4). Pure, total,
/// no I/O — the natural unit-test surface for §13 invariant 11 and §7.1's feat/fix/perf table.
///
/// - `breaking == true` → `Severity::Major`, regardless of `commit_type` (a commit can be
///   `fix!: …` and still be major — the `!`/footer overrides the type-implied severity, per
///   the Conventional Commits spec itself).
/// - `commit_type == "feat"` (exact, lowercase) → `Severity::Minor`.
/// - `commit_type == "fix" | "perf"` (exact, lowercase) → `Severity::Patch`.
/// - anything else (`chore`, `docs`, `refactor`, `test`, `ci`, `build`, `style`, any typo'd
///   or unrecognized type) → `Severity::None`. Unrecognized types are *not* an error — a
///   changelog-worthy `type` vocabulary is intentionally not enumerated anywhere in this
///   crate; only the three that carry version weight are named.
pub fn raw_severity(commit: &ConventionalCommit) -> Severity;

/// `ParsedCommit::NonConventional` always contributes `Severity::None` — folded in here so
/// callers never have to match on `ParsedCommit` themselves.
pub fn raw_severity_of(commit: &ParsedCommit) -> Severity {
    match commit {
        ParsedCommit::Conventional(c) => raw_severity(c),
        ParsedCommit::NonConventional { .. } => Severity::None,
    }
}
```

### C.4 No pre-major policy here — by design

This crate does **not** implement §7.1's pre-major remap. `bump_version`'s rigidity (§6.2,
§F.6.2, P1) was never in question — the interesting design point is that
`callisto_conventional::infer_severity` (§C.7) doesn't apply the remap either, even though an
earlier draft of this section did (a `PreMajorInferencePolicy` type and an
`apply_pre_major_policy` function, both since moved). Bump-decision policy is coordination
logic; P6 puts coordination logic in `callisto-graph`, not in the crate whose job is
classifying commits. `infer_severity` returns a **raw**, unremapped `Severity` — `callisto-
graph`'s `CommitInference` adapter (§G.6.4) applies `apply_pre_major` (§G.6.3) to that raw
value immediately after receiving it, before it ever becomes a `BumpReason::Inference`. This
crate has no `PreMajorInferencePolicy` field anywhere in its types and no dependency on the
concept beyond `InferenceInput::current_version`/`has_prior_release`, which it forwards
without interpreting (§C.7's gate reasoning — the `0.0.z`/no-prior-release inert cases and
the SemVer-only scope of the gate — is documented at §G.6.3, alongside the function that
actually applies it).

### C.5 Commit fetching — `window.rs`

```rust
use callisto_model::{CommitSha, CommitWalker};
use std::path::PathBuf;

/// The lower bound of the commit range to scan. An **exclusive** bound when `SinceCommit` —
/// `git log <sha>..HEAD`, matching `..` range semantics — and the case §7.1 calls "since last
/// tag" or §8 calls "since the pre-cursor ref," depending on which `CommitSha` the caller
/// resolved (§C.6 for the pre-cursor half; the stable-tag half is `callisto-graph`'s, via
/// `select_last_tag`, §M.9.4).
///
/// This type deliberately does not distinguish *which kind* of ref produced the `CommitSha`
/// — a tag sha and a pre-cursor sha are fetched identically by `git log`. What differs
/// between "since stable tag" and "since pre-cursor" is entirely upstream, in how the caller
/// resolves `has_prior_release` for §C.4's gate (§8: a pre-cursor ref is bookkeeping about
/// what inference already counted, not a release signal) — that distinction belongs at the
/// call site, not baked into this enum.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InferenceWindow {
    SinceCommit(CommitSha),
    /// No lower bound — `git log HEAD -- <pathspecs>`, the whole visible history. The routine
    /// bootstrap case (P2: a package with no prior tag is not an error state).
    FullHistory,
}

/// Runs `git log` over `window`, scoped to `pathspecs` (workspace-root-relative — resolved
/// against whatever root `walker` itself carries internally, e.g. `GitAccess::discover`'s
/// `root`; see the "no `cwd` parameter" note below), and parses every resulting commit via
/// [`parse_commit`]. Order is git's own reverse-chronological log order; callers that need a
/// count do not care about order, and nothing in this crate's output is order-sensitive.
///
/// **Excludes merge commits** (`--no-merges`). A merge commit's own subject ("Merge pull
/// request #123 from …") is never conventional, and on a workflow that squash-merges this has
/// no effect either way; on a workflow that uses real merge commits, excluding them avoids
/// counting an auto-generated subject as a `Severity::None` no-op line that would otherwise
/// just inflate `commit_count` for no informational gain.
///
/// Sourcing the history is entirely `walker`'s business — this crate names only the Layer 1
/// [`callisto_model::CommitWalker`] contract (`callisto-model/src/commit.rs`), so it links
/// against no VCS engine at all: callers hand it native gix, a shelled-out `git`,
/// `callisto-vcs`'s gix-with-shell-fallback `GitAccess` selector, or a test double, and the
/// delimiter parsing below is identical either way. `CommandRunner`-shelling `git log`
/// directly used to live here; it moved behind this trait so this crate has no
/// *production* dependency on `callisto-vcs` at all (§C.0's dependency table lists only
/// `callisto-model`; `callisto-vcs` appears only as a **dev**-dependency exercising the real
/// `GitAccess` backend in tests, e.g. `test_real_git_access_backend_satisfies_commit_walker...`
/// below).
pub fn fetch_commits(
    walker: &dyn CommitWalker,
    window: &InferenceWindow,
    pathspecs: &[PathBuf],
) -> Result<Vec<ParsedCommit>, ConventionalError>;
```

`&dyn CommitWalker` rather than a generic parameter is a deliberate, compatible choice: it
keeps the trait dyn-compatible (mirroring `CommandRunner`'s own dyn-compatible design, §M.10)
precisely so that any `&GitAccess`/`&GitRepository`/`&ShellGit` (all three implement
`CommitWalker`, §V.7) coerces here with no adapter. Neither form is privileged. Note there is
no `cwd: &Path` parameter here — unlike the `CommandRunner`-based
functions elsewhere in this crate (§C.6), a `CommitWalker` implementation already carries its
own repository root internally (e.g. `GitAccess::discover`'s `root`), so this function has no
separate root to be told.

### C.6 Pre-mode cursor ref — `pre_cursor.rs`

The git-ref half of §8's `refs/callisto/pre-cursor/<PackageId>` mechanism. It lives here, not
in `callisto-graph`, because it is bookkeeping specific to what conventional-commit inference
has already counted — the ref's *only* consumer is this crate's own windowing, and keeping its
read/write together with the thing it exists to make idempotent is the smallest correct
boundary. `callisto-graph` calls these functions at the appropriate point in §7.6's mutation
ordering (§G.10.2 step 8); it does not reimplement ref resolution.

```rust
use callisto_model::PackageId;

/// `refs/callisto/pre-cursor/<display_name>`, e.g. `refs/callisto/pre-cursor/@myorg/cli`.
/// Pure — no I/O, fixturable from a bare `PackageId`. Uses `PackageId::display_name`
/// (§M.15's table entry for this ref namespace: "its *name* is derived from
/// `PackageId::display_name`, so [callisto-model] supplies the string; the ref itself is I/O").
///
/// Living outside `refs/tags/` means this can never collide with, or be mistaken for, a real
/// release tag (§8) — no additional namespacing precaution is needed beyond the `pre-cursor/`
/// path segment itself.
pub fn pre_cursor_ref_name(package: &PackageId) -> String;

/// `git rev-parse --verify --quiet <pre_cursor_ref_name>`. Returns `Ok(None)` when the ref
/// does not exist — the routine case for a package's first-ever pre-mode cycle, or any
/// package that has never gone through pre-mode with `Auto` trigger — not an error (mirrors
/// `last_tag_for`'s "no matching tag" outcome, §M.9.3). Returns `Ok(Some(sha))` when it
/// resolves. Any other outcome (git present but the ref is corrupt, or the repository itself
/// is unusable) is `Err(ConventionalError::MalformedPreCursorRef)`.
pub fn resolve_pre_cursor(
    runner: &dyn CommandRunner,
    cwd: &Path,
    package: &PackageId,
) -> Result<Option<CommitSha>, ConventionalError>;

/// `git update-ref <pre_cursor_ref_name> <sha>`. Called by `callisto-graph` "at the moment
/// [`version`] computes a pre-mode bump for an `Auto`-trigger package, alongside the manifest
/// write (same mutation phase, §7.6)" (§8) — i.e. this function does not decide *when* to
/// advance the cursor, only performs the write once the caller has decided to. Unconditional
/// overwrite (no compare-and-swap): §8 gives this ref no concurrent-writer story beyond what a
/// single `version` invocation already serializes, and a stale advance is self-correcting on
/// the next run since the ref only ever moves forward with real commits.
pub fn advance_pre_cursor(
    runner: &dyn CommandRunner,
    cwd: &Path,
    package: &PackageId,
    sha: &CommitSha,
) -> Result<(), ConventionalError>;
```

Pushing these refs is the calling workflow's job, via a wider ref-spec on the `git push --tags`
step it already has (§8) — no callisto command pushes anything (§13 inv. 16, §13 inv. 24).

### C.7 Top-level entry point — `infer.rs`

```rust
use std::path::PathBuf;

use callisto_model::{CommitWalker, PackageId, Severity, Version};

/// Everything §7.1/§8's inference needs about one package, for one call. Deliberately flat —
/// this crate has no `Package` type to destructure one from (§C.0), so a caller in
/// `callisto-graph` builds this from its own `Package` plus whatever it already resolved
/// (last-tag sha, pre-cursor sha). No pre-major policy field — see `InferredSeverity::severity`
/// below for why.
pub struct InferenceInput<'a> {
    /// Used only to compute the pre-cursor ref name if the caller needs
    /// `pre_cursor_ref_name`/`resolve_pre_cursor`/`advance_pre_cursor` — `infer_severity`
    /// itself does not read or write the pre-cursor ref; the caller resolves `window`
    /// (possibly via `resolve_pre_cursor`) *before* calling this function and supplies the
    /// already-resolved `CommitSha` inside it. Kept on this struct anyway so a single
    /// `InferenceInput` is what a caller threads through both the cursor lookup and the
    /// inference call, rather than passing `PackageId` twice.
    pub package: &'a PackageId,
    pub pathspecs: &'a [PathBuf],
    pub window: InferenceWindow,
    pub current_version: &'a Version,
    /// Whether this package has ever had a stable release tag — resolved by the caller from
    /// `last_tag_for`/`select_last_tag` (§M.9.4, §G.9.1), independent of `window`: a pre-mode
    /// `SinceCommit` window can still be this package's first-ever release cycle, so
    /// `window`'s shape alone cannot answer this (§C.4's gate).
    pub has_prior_release: bool,
}

/// The result §7.1/§8 need, shaped to construct `BumpReason::Inference { commits, remapped }`
/// (§M.12.3) directly — this crate's return type and that variant's fields are the same two
/// facts for a reason: computing the attribution *is* computing the inference, so there is no
/// second pass that re-derives `commits`/`remapped` from a discarded intermediate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InferredSeverity {
    /// **Raw** — no pre-major remap applied. §C.4: that policy lives entirely in
    /// `callisto-graph` (§G.6.3), applied by the caller immediately after this function
    /// returns; this crate has no `PreMajorInferencePolicy` field to apply it with even if
    /// it wanted to.
    pub severity: Severity,
    /// Total commits in the window, conventional or not — the number a human-readable
    /// attribution line reports ("inferred from 6 commits"), not a "conventional commits
    /// only" count, since a package with 6 commits where only 1 is a `fix:` should still say
    /// 6 (the other 5 were considered and correctly contributed nothing).
    pub commit_count: usize,
    /// Every commit in the window, in git's log order — carried so `callisto-changelog` can
    /// itemize `ChangeSource::Commit` bullets (§CL.3) without a second `git log`. Callers
    /// that only need a severity ignore it.
    pub commits: Vec<ParsedCommit>,
}

/// Fetches the window's commits, classifies each (§C.3), and aggregates by max (`None <
/// Patch < Minor < Major`, the same ordering §M.5's `Severity` is fixtured against, §M.17
/// item 4). Returns the raw aggregate — no pre-major remap (§C.4).
///
/// A changeset always wins over inference (§7.1) — enforced by the *caller*
/// (`callisto-graph`'s aggregation step, §G.6.2), not here: this function has no visibility
/// into whether a changeset named the package, and folding that check in here would require
/// this crate to depend on `callisto-format`'s changeset type for no benefit — inference is
/// unconditional and cheap, and "ignore this result if a changeset exists" is a one-line
/// caller-side check.
pub fn infer_severity(
    walker: &dyn CommitWalker,
    input: &InferenceInput<'_>,
) -> Result<InferredSeverity, ConventionalError>;
```

### C.8 Errors — `error.rs`

```rust
use callisto_model::{CommandError, CommitWalkError};

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConventionalError {
    /// `resolve_pre_cursor`/`advance_pre_cursor`'s `CommandRunner`-shelled `git` calls
    /// (§C.6) failing outright — the process could not be run at all (`git` missing, WASM
    /// host with no `exec_command`, etc.) — propagated as-is from `CommandRunner::run`.
    #[error(transparent)]
    Command(#[from] CommandError),

    /// Any failure reported by the [`CommitWalker`] backing `fetch_commits`/`infer_severity`
    /// (§C.5, §C.7) — `git log` itself failing, an unparsable log stream, or an
    /// explicitly-requested `since` ref that doesn't resolve to a commit. Named for the
    /// Layer 1 contract, not for whichever VCS engine happens to satisfy it: this crate
    /// parses conventional commits and has no opinion on where the history came from.
    #[error(transparent)]
    CommitWalk(#[from] CommitWalkError),

    /// `git rev-parse --verify` on a pre-cursor ref exited non-zero for a reason other than
    /// "ref does not exist" (§C.6) — e.g. an ambiguous or corrupt ref.
    #[error("pre-cursor ref `{ref_name}` in `{cwd}` could not be resolved: {stderr}")]
    MalformedPreCursorRef { cwd: PathBuf, ref_name: String, stderr: String },

    /// `git update-ref` on a pre-cursor ref exited non-zero.
    #[error("failed to advance pre-cursor ref `{ref_name}` in `{cwd}` to `{sha}`: {stderr}")]
    PreCursorAdvanceFailed { cwd: PathBuf, ref_name: String, sha: String, stderr: String },
}
```

`Command` and `CommitWalk` cover the two different subprocess-facing surfaces this crate has:
§C.6's `resolve_pre_cursor`/`advance_pre_cursor` still shell `git` directly via `CommandRunner`
(unlike `fetch_commits`/`infer_severity`, which moved behind `CommitWalker`, §C.5/§C.7), so a
`CommandRunner::run` failure from *those* two functions surfaces as `Command`, while a `git
log`/history-walk failure from `fetch_commits`/`infer_severity` surfaces as `CommitWalk` —
distinct variants because they're reached through genuinely distinct call paths, not two names
for the same condition. `CommitWalkError` (`callisto-model/src/commit.rs`) itself narrows a much wider
set of possible backend failures (gix, `ShellGit`, or any other `CommitWalker` implementation)
down to three: `Command` (the walk's own subprocess couldn't run), `RefNotFound` (an explicit
`since` ref didn't resolve), and `Backend` (everything else, carrying the backend's own
rendering) — see `callisto-vcs`'s §V.7 for how a `VcsError` narrows into this contract.

`callisto-graph` wraps this transparently (`GraphError::Conventional`, §M.13.3), reachable
only when its `inference` feature is on.

### C.9 Fixture obligations

Per §12.6's "broader than JSON shape alone," and this crate having no JSON output of its own
(its output feeds `callisto-graph`'s attribution, not stdout directly):

1. **Header grammar corpus** — one fixture per accepted/rejected shape: `feat: x`,
   `feat(scope): x`, `feat!: x`, `feat(scope)!: x`, `fix:x` (rejected — no space), `: x`
   (rejected — empty type), `feat: ` (rejected — empty description), `Feat: x` (accepted as
   `ParsedCommit::Conventional`, classifies `Severity::None` per §C.2.2's decision).
2. **Footer/breaking corpus** — `BREAKING CHANGE: x`, `BREAKING-CHANGE: x`,
   `Breaking Change: x` (not detected — case rule, §13 inv. 11), a footer with a multi-line
   continuation value, a commit with both a `!` and a `BREAKING CHANGE:` footer (still just
   one `breaking: true`), a commit with neither.
3. **Severity table corpus** — one fixture per `raw_severity` row in §7.1's table plus
   `NonConventional`, asserting the exact `Severity` produced.
4. *(No pre-major boundary corpus here.)* This crate returns a raw, unremapped severity
   (§C.4) — the pre-major policy and its fixture obligations now live entirely in
   `callisto-graph` (§G.15 item 10d), against `apply_pre_major` directly.
5. **Pre-cursor ref corpus** — `pre_cursor_ref_name` against a representative `PackageId`
   corpus (bare, prefixed, scoped npm name) with no I/O, and a
   `resolve_pre_cursor`/`advance_pre_cursor` round-trip fixture against `callisto-fixtures`'
   in-memory `CommandRunner` test double (§CF.3.4).
6. **`wasm32-wasip1` run** — this crate's suite runs under the same CI job as
   `callisto-model`'s (§0.1 rule 2), using the fixture `CommandRunner` — this crate performs
   no I/O of its own outside that trait, so nothing here is WASI-sensitive beyond what the
   trait boundary already covers.

### C.10 Index of `[SPEC DECISION]` flags

| # | Section | Decision |
|---|---|---|
| 1 | §C.2.2 | Type-token classification is case-sensitive, exact-lowercase-match; parsing itself is case-preserving. |
| 2 | §C.2.3 | Footer-block detection uses a simplified "first matching line starts the footer block" rule, sufficient for breaking-change detection, not a fully RFC-822-conformant footer parser. |
| 3 | §C.4 | Pre-major policy is applied once to the aggregated severity, not per-commit before aggregation (proven equivalent for this transform table). |
| 4 | §C.4 | The pre-major gate is defined only for `VersionGrammar::SemVer`; calling it for a non-SemVer version is a caller bug, not a modeled error case. |

---

## 6. `callisto-changelog`

**Purpose.** Turn "this package was bumped, for these reasons" into a Markdown section, and
prepend it to that package's `CHANGELOG.md`.

**License:** AGPL-3.0 (§16). **Milestone:** v0.1 (§17 — "previously unscheduled despite v0.1's
`version` command needing it": §7.6 step 7 is a mutation step of `version`, so this crate ships
alongside `callisto-graph`/`callisto-manifests`, not later).

### CL.0 Dependencies

| Edge | Kind | Why |
|---|---|---|
| `callisto-changelog → callisto-model` | normal | `PackageId`, `Version`, `Severity`, `GroupName`, `GroupKind`, `DepKind`, `CommitSha`, `Diagnostic` |
| `callisto-fixtures` | **dev** | §CL.9 |

Nothing else `callisto-*`. No moon dependency (§13 inv. 26); builds for `wasm32-wasip1` and
runs its fixture suite under `wasmtime` with only the workspace root preopened (§0.1 rule 2)
— this crate's only I/O is plain UTF-8 file read/write under the workspace root, which is
exactly the WASI-safe shape `std::fs` handles correctly under a preopened directory (unlike
process exec, so §M.10's `CommandRunner` seam is not needed here at all).

### CL.1 What this crate does, and does not, own

**Owns:** turning a `ChangelogInput` into (a) a Markdown section prepended to a package's
`CHANGELOG.md`, and (b) the same Markdown as a `String` handed back for reuse in
`PublishPlan.releases[].changelogSection` (§9.2, §M.12.2) and in `compose-pr-body`'s
per-package `<details>` block (§12.2).

**Does not own:**

- *Deciding what the reasons are.* Severity aggregation, cascade, and group union all happen
  in `callisto-graph` (§7, §G.6, §G.7).

  > `[SPEC DECISION, not in 00-design.md: `ChangelogInput` (§CL.3) is a distinct, richer type
  > from `BumpReason`, assembled by `callisto-graph` during aggregation (§7.1) before it
  > collapses to the single summarized `reason` field `VersionReport` carries.]` §M.12.3
  > documents `BumpRecord.reason` as one `BumpReason`, not a `Vec`; a changelog that rendered
  > only the dominant reason would silently drop every other changeset naming a
  > multiply-authored bump, which contradicts §6.1's "summary paragraph" being the whole point
  > of a changeset. This is the smallest fix consistent with P1's byte-compat spirit — the
  > changeset text itself must survive into the changelog, verbatim, or the crate has failed
  > at its one job.

- *Commit/PR linking.* `@changesets/cli`'s `changelog` config key selects a pluggable
  generator, most commonly `@changesets/changelog-github`, which prefixes each bullet with a
  commit hash and links a PR number via the GitHub API. §18 Q4's migration table already
  resolves this: that key has **no equivalent** in `callisto.toml` and is dropped with a
  warning, because `callisto-changelog` is a single built-in generator, not a plugin host
  (§14 has no changelog-selection key for the same reason), and §9.5 removed all
  HTTP/GitHub-API dependencies from callisto's tree entirely. This crate's default output
  matches `@changesets/cli`'s own **built-in, non-GitHub** default generator shape — plain
  summary text, no link — which is the one part of changesets' changelog behaviour that has
  no external-service dependency and is therefore the only one worth being consistent with.
  (The short commit sha in §CL.5's `Commit` bullet is read off `CommitSha::short`, not
  fetched.)
- *Byte-compatibility.* P1's guarantee is scoped to `.changeset/*.md` and `pre.json` only
  (§4's scope note) — `CHANGELOG.md` is not part of it. This crate borrows changesets'
  heading conventions (`## <version>`, `### Major Changes` / `### Minor Changes` /
  `### Patch Changes`) because they are familiar to every migrating user and there is no
  reason to invent new vocabulary for the same concept, not because anything requires it.
- *Rendering `.bumps[].governedBy` attribution lines.* That is `callisto-cli`'s (§CLI.5.3) —
  a CLI-output concern, not a changelog-content concern. This crate never reads `ConfigKey`
  and has no dependency on it.

### CL.2 Where this sits in the pipeline

§7.6's mutation phase, with this crate's role made explicit:

```text
3. Write canonical manifests
4. Write platform manifests
5. Update optionalDependencies
6. Rewrite dependency specs (cascade)
7. Prepend changelog entries      ← callisto-changelog, this crate
8. Delete consumed changeset files
```

Step 7 runs after every version-bearing write (3–6) and before changeset deletion (8) — by the
time this crate runs, every package's `to` version is final and every cascade decision has
already fired, but the changeset files this crate's *input* was built from still exist on disk.

**Called from two places, not one**, both driving the same pure `render_section` (§CL.5):

1. **`version`'s mutation phase (step 7)** — real input, rendered by `render_section` and
   written by `prepend` (§CL.6, which does real file I/O). The rendered text is **not**
   retained in memory for `plan-publish`'s `changelogSection`: that is a separate process
   invocation and reads the section back off disk instead (§CL.7.1).
2. **`compose-pr-body` (§12.2, runs *before* `version`, §13 invariant 23)** — a preview call:
   the same machinery, built from pending (not-yet-consumed) changesets and a non-mutating
   `plan_version` computation (§G.11) that supplies real prospective `to` versions, rendered
   through the same `render_section`, but **never written to `CHANGELOG.md`**. This is why
   `render_section` is a pure `&ChangelogInput -> Result<String, _>` function with no file I/O
   in it: the write half (§CL.6's `prepend`, which does real file I/O) is separately callable,
   precisely so `compose-pr-body` can use the render half alone.

> `[SPEC DECISION, not in 00-design.md: one render function, three consumers —
> `CHANGELOG.md`'s prepended section, `PublishPlan.releases[].changelogSection`, and
> `compose-pr-body`'s `<details>` block all trace back to one `render_section` call, never to
> three independent renderings. (`changelogSection` reaches it indirectly: `plan-publish` runs
> in a later process and reads back the section `render_section` wrote, via `extract_section`
> — §CL.7.1. That is still one rendering, transported through the file rather than through
> memory, which is the only transport available across a process boundary.)]` §9.2 names
> `changelogSection` and §12.2 names the PR body's
> per-package section but never states they share an implementation; keeping them separate
> would let `CHANGELOG.md` content drift from PR-body content for the same release, which is
> exactly the duplicated-computation P6 forbids. The only differences between the call sites
> are *when* they run and *whether the result is written to disk*, not *how the content is
> computed*.

### CL.3 Input types — `input.rs`

```rust
use callisto_model::{CommitSha, DepKind, GroupName, PackageId, Severity, Version};

/// Everything this crate needs to render one package's changelog section for one bump.
/// Built by `callisto-graph` during aggregation (§7.1, §G.6.9) — this crate performs no
/// aggregation, no cascade computation, and no changeset parsing of its own; it only formats
/// what it is given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangelogInput {
    pub package: PackageId,
    pub from: Version,
    /// The version this section is being written for. `Some` in every ordinary case,
    /// including `compose-pr-body`'s preview — that call site computes real prospective
    /// versions via `plan_version` (§G.11), because §12.2's
    /// `<summary>package@version</summary>` shape exists so a reviewer can see what version
    /// they are approving.
    ///
    /// `None` is the residual case only: a package with contributing entries but no
    /// computable target version (an all-`none`-severity changeset, §6.1). `render_section`
    /// renders `## Unreleased` for it (§CL.5).
    pub to: Option<Version>,
    /// Every entry that contributed to this bump, in the order they should render. Ordering
    /// within a severity bucket is the caller's responsibility (deterministic — sorted
    /// changeset filenames, then git-log-order commits, then `PackageId`-sorted cascade
    /// entries) — this crate does not re-sort, it renders in the order given, once grouped by
    /// severity (§CL.4).
    pub entries: Vec<ChangelogEntry>,
}

/// One contributing item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangelogEntry {
    /// The severity bucket this entry renders under. **`Severity::None` entries must never
    /// reach this type** — filtering them out is the aggregation step's job (§CL.4). This
    /// includes both §6.1's human-authored `none`-severity changesets (a documented no-op, by
    /// definition never bumps anything and therefore never contributes changelog content) and
    /// §7.4's cascade-table `Severity::None` outcome for an out-of-range dev-dependency (a
    /// spec rewrite with no version bump). Both are the same enum value; both are equally
    /// excluded, by the same filter, for the same reason — there is nothing severity-specific
    /// about "is this content-bearing," so one rule covers both without special-casing either.
    pub severity: Severity,
    pub source: ChangeSource,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChangeSource {
    /// A changeset named this package (§7.1, §6.1). `summary` is the changeset body's text,
    /// already trimmed of leading/trailing whitespace — verbatim otherwise.
    Changeset { filename: String, summary: String },

    /// A conventional commit inferred this package's bump (`ReleaseTrigger::Auto`, §7.1).
    /// One entry per contributing commit — `callisto-conventional` supplies the full subject
    /// line (§C.7's `InferredSeverity::commits`), not just a count, specifically so this crate
    /// can itemize rather than collapse to "N commits" (contrast with
    /// `BumpReason::Inference { commits: usize, .. }`, which *is* a collapsed count because
    /// the JSON report has no per-commit itemization need — §CL.1).
    Commit { sha: CommitSha, subject: String },

    /// §7.4's cascade fired for this package because a dependency edge stopped covering.
    DependencyUpdate { dependency: PackageId, dep_kind: DepKind, to: Version },

    /// §7.4's peer-escalation row fired: an out-of-range non-patch peer source escalated this
    /// package to major (§13 invariant 9). Kept distinct from `DependencyUpdate` because it is
    /// the one cascade outcome whose severity (major) can differ from a
    /// `Runtime`/`Optional`/`Build` cascade's (patch) for the *same* dependency name across
    /// two packages in one run, and the rendered line says "peer dependency," not just
    /// "dependency," so the reader understands why a peer bump was major.
    PeerEscalation { dependency: PackageId, to: Version },

    /// §7.1's fixed-group severity union, or §7.5's linked-group joint-naming union, bumped
    /// this package without it owning any changeset or inferred commit of its own — the common
    /// case for a fixed-group sibling that moved only because something *else* in its group
    /// was named. Renders as a single explanatory line rather than leaving the section empty
    /// (§CL.5) — an empty `## <version>` heading with no content under it reads as a bug, not
    /// a deliberate "nothing to say," so this line exists specifically to prevent that.
    GroupUnion { group: GroupName, kind: GroupKind },

    /// §7.5's new-member exemption: this package joined a fixed group and was force-set to
    /// the group's target version on first inclusion, with no divergence check and no
    /// authored content of its own — a real version bump (usually from a placeholder like
    /// `0.0.0`) that still needs a changelog line explaining why the number moved.
    NewGroupMember { group: GroupName },
}
```

`GroupKind` (`Fixed`/`Linked`) is `callisto_model::GroupKind` — not redefined here (§M.2).

> `[SPEC DECISION, not in 00-design.md: `ChangeSource` has exactly these six variants, a
> subset of `BumpReason`'s eight.]` `BumpReason::PreRelease { tag }` (§8's pre-mode counter) is
> deliberately **not** a `ChangeSource` — pre-mode never fires in isolation; the real cause is
> always one of the six variants above, and pre-mode only changes which *version number* that
> cause produces (`pre.0` vs. a real release), not *what content* the changelog should show.
> `render_section` takes the already-resolved `to: Option<Version>` — including a prerelease
> version string when relevant — so it needs no separate pre-mode branch.

### CL.4 Severity grouping — the rule, made structural

```rust
/// Groups a `ChangelogInput`'s entries into the three renderable buckets, in render order.
/// Buckets with zero entries are simply absent from the result — `render_section` (§CL.5)
/// never emits an empty `### <X> Changes` heading.
///
/// `Severity::None` entries are rejected here, not silently dropped: constructing a
/// `ChangelogEntry` with `severity: Severity::None` is a caller bug (§CL.3's doc comment says
/// why one should never exist), and returning an error rather than quietly discarding it means
/// that bug surfaces as a test failure in `callisto-graph`'s aggregation code, not as a
/// silently-thinner changelog nobody notices.
pub fn group_entries(entries: &[ChangelogEntry]) -> Result<GroupedEntries<'_>, ChangelogError>;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GroupedEntries<'a> {
    pub major: Vec<&'a ChangelogEntry>,
    pub minor: Vec<&'a ChangelogEntry>,
    pub patch: Vec<&'a ChangelogEntry>,
}
```

### CL.5 Rendering — `render.rs`

```rust
/// Pure. No file I/O, no `CommandRunner` — the WASM-safety property this crate leans on is
/// simply "this function touches no external state at all." Called identically by `version`'s
/// mutation phase and by `compose-pr-body`'s preview (§CL.2).
///
/// Output shape (heading levels chosen to nest under a package's `# <name>` file header,
/// §CL.6):
///
/// ```markdown
/// ## 1.3.0
///
/// ### Major Changes
///
/// - Widen the public `Foo` trait to accept async closures.
///
/// ### Patch Changes
///
/// - Fix a panic when `bar()` is called with an empty slice.
/// - Dependency updates
///   - `@myorg/sdk` → `2.0.0`
///   - `@myorg/utils` → `1.4.1`
/// ```
///
/// When `input.to` is `None` — a package with contributing entries but no computable target
/// version (§CL.3) — the heading renders as `## Unreleased` instead of a version number,
/// matching the one case where changesets' own convention already has a name for "a version
/// that does not exist yet." This is **not** the normal `compose-pr-body` path: that call
/// site supplies a real prospective version (§G.11's `compose_pr_body`, §CL.7.2).
pub fn render_section(input: &ChangelogInput) -> Result<String, ChangelogError>;
```

Bullet text per `ChangeSource` variant (all rendered as `- <text>`, at 2-space nested indent
for the `DependencyUpdate` sub-list):

| Variant | Rendered text |
|---|---|
| `Changeset { summary, .. }` | `summary`, verbatim |
| `Commit { subject, sha }` | `` `subject` (`{sha.short()}`) `` — 7 characters, matching `git`'s own default abbreviation length |
| `DependencyUpdate { .. }` (≥1 in a section) | one parent bullet `Dependency updates`, followed by one nested `` `{dependency}` → `{to}` `` line per entry, all `DependencyUpdate`s in that section collapsed under the single parent bullet |
| `PeerEscalation { dependency, to }` | `` Peer dependency `{dependency}` requires `{to}` `` — rendered standalone, not nested under `DependencyUpdate`'s parent bullet, per §CL.3's rationale for keeping the variant distinct |
| `GroupUnion { group, kind }` | ``Released together with the `{group}` {fixed \| linked} group.`` |
| `NewGroupMember { group }` | ``Joined the `{group}` group at this version.`` |

> `[SPEC DECISION, not in 00-design.md: the exact bullet-text templates above.]` Neither the
> design doc nor P1 constrains changelog prose. These are kept close to `@changesets/cli`'s own
> wording (`Dependency updates` mirrors its `Updated dependencies`) specifically so a migrating
> user's muscle memory for reading a changesets-generated `CHANGELOG.md` transfers with minimal
> friction — the same adoption-friction reasoning P1 applies to the file formats it does
> guarantee, applied here as a *design preference*, not a *compatibility requirement*.

### CL.6 Writing — `write.rs`

```rust
/// Prepends `rendered` (the output of `render_section`) to the package's `CHANGELOG.md`,
/// creating the file with a `# <display_name>` header if it does not exist yet. Never touches
/// any earlier section — this is a pure prepend, matching every other changelog-generating
/// tool's newest-first convention.
///
/// `root` is the workspace root (an already-resolved absolute path supplied by the caller —
/// this crate does not resolve it); `changelog_path` is `Package.changelog`'s value (§5.1,
/// §M.6.1), workspace-root-relative per §M.1.3.
///
/// Plain `std::fs` read/write — no format-preserving-editor machinery is needed the way
/// `callisto-manifests`' TOML/JSON writers need one (§CM.5.1), because there is no existing
/// structure to preserve: a prepend to a Markdown file is byte-identical whether or not
/// anything already understands the file's contents.
pub fn prepend(
    root: &Path,
    changelog_path: &Path,
    display_name: &str,
    rendered: &str,
) -> Result<(), ChangelogError>;
```

```rust
/// The read-back half of §CL.7.1: given a whole `CHANGELOG.md` and a version, return the
/// section this crate's own `render_section` + `prepend` wrote for it, or `None`.
///
/// **Deterministic, and deliberately narrow.** The rule is exactly the inverse of §CL.5's
/// output shape, and nothing more:
///
///   1. Find the first line that is exactly `## <version.render()>` (after trimming trailing
///      whitespace). No fuzzy matching, no `v` prefix tolerance, no scanning for a heading
///      that merely *contains* the version — a changelog a human has hand-edited into a
///      different shape yields `None`, which the caller handles (§CL.7.1), rather than a
///      plausible-looking wrong slice.
///   2. The section runs from the line after that heading up to (exclusive) the next line
///      beginning with `## ` or `# ` at column 0, or end of file.
///   3. Return that slice with leading and trailing blank lines trimmed. An empty result
///      after trimming is `None`, not `Some("")`.
///
/// A fenced code block containing a `## …` line inside a changelog body would end the section
/// early under rule 2. That is accepted rather than defended against: `render_section` never
/// emits a fenced block (§CL.5's bullet templates are all single-line), so the only way to
/// reach it is a hand-edited changelog, which rule 1's strictness already treats as
/// out-of-contract.
pub fn extract_section<'a>(changelog: &'a str, version: &Version) -> Option<&'a str>;
```

**`Package.changelog: Option<PathBuf>` — `None` means opt-out, not "use a default location."**
A package with `changelog: None` gets no `CHANGELOG.md` write at all; §CL.2's step 7 skips it
entirely. (`compose-pr-body`'s preview may still render it in-memory for the PR body, since a
PR body summarizing "no changelog file, but here is what changed" is still useful — that is a
`callisto-graph`/`callisto-cli` composition choice, not this crate's concern.)

> `[SPEC DECISION, not in 00-design.md: `Package.changelog: None` is opt-out, not "resolve a
> default path."]` §5.1 declares the field `Option<PathBuf>` without saying what `None` means;
> §17 v0.1 commits `callisto init` to scaffolding config, so the natural reading is that `init`
> populates a sensible default (`<package-root>/CHANGELOG.md`) for every package it discovers,
> and `None` is reachable only when a user has explicitly edited it out — the smallest
> interpretation consistent with P5: an explicit opt-out is a legitimate, structurally
> representable choice; an implicit fallback path this crate would have to reconstruct from
> `ManifestDecl`'s canonical-manifest location is not.

### CL.7 Feeding `changelogSection` and the PR body

#### CL.7.1 `plan-publish` reads the section back off disk

> `[SPEC DECISION, not in 00-design.md: `ReleaseEntry.changelog_section` is produced by
> `extract_section` reading the already-written `CHANGELOG.md`, and is an **optional** JSON
> field, absent for a package with `changelog: None`.]` §M.12.2's field doc said it is
> "produced by `render_section`, never re-rendered independently," and an earlier draft of
> this section offered re-rendering or re-reading as equally acceptable. Neither survives
> contact with the process boundary: `plan-publish` is a **separate process invocation**,
> after `version` already ran, deleted the consumed changesets (§7.6 step 8) and had its
> output committed by the calling workflow (§9.3). There is therefore no in-memory
> `ChangelogInput` to reuse, and rebuilding one is not merely inconvenient but *impossible* —
> the changeset files it was built from are gone, and §9.2 pins `releases[].sha` to HEAD at
> `plan-publish` time specifically because that commit is the one containing `version`'s
> output. Reading the section back out of the file `version` wrote is the only mechanism that
> is a pure function of on-disk state at `plan-publish` time, which is what §0.1 rule 3's
> compute/apply split requires of a plan function. The extraction rule is pinned in §CL.6 so
> that "read it back" is a specified operation rather than an implementation's guess.

The path, end to end:

```
version:        render_section(input) ──► prepend(root, changelog_path, name, rendered)
                                              writes `## <to>` + body into CHANGELOG.md
plan-publish:   read CHANGELOG.md ──► extract_section(&contents, &released_version)
                                              ──► ReleaseEntry.changelog_section
```

Three cases, all of which `callisto-graph` must handle explicitly (§G.11):

| Case | `changelogSection` |
|---|---|
| `Package.changelog = Some(path)`, section found | the extracted section |
| `Package.changelog = None` (§CL.6's opt-out) | **absent** — no file was ever written, so there is nothing to read, and emitting `""` would misreport "this release has no notes" as if it were a fact about the release rather than about the configuration |
| `changelog = Some(path)` but the file is missing, or `extract_section` returns `None` (hand-edited heading, a release whose section a human deleted) | **absent**, plus `DiagnosticCode::ChangelogSectionNotFound` (§M.11.2) — a warning, never a hard failure: a missing release note must not block a publish plan |

`ReleaseEntry.changelog_section` is therefore `Option<String>` with
`skip_serializing_if = "Option::is_none"` (§M.12.2) — an **optional** field under §12.5's
contract, length-gated rather than hard-gated, so a consumer that already reads it keeps
working and one that does not is unaffected.

#### CL.7.2 `compose-pr-body`

Wraps `render_section`'s output in the
`<details><summary>package@version</summary>…</details>` shape §12.2 specifies, once per
package with pending changes. The `version` in that summary is the **prospective** version
`plan_version` computed, not a placeholder — see §G.11's `compose_pr_body` and §CL.5's
`to: Option<Version>` note.

This crate exposes no `ReleaseEntry` or PR-body-shape type of its own — those are
`callisto-model`'s (§M.12.2) and `callisto-graph`'s (§G.11) concerns respectively; this crate's
only public surface toward them is the `String` from `render_section` and the `&str` from
`extract_section`.

### CL.8 Errors — `error.rs`

```rust
/// Deliberately small — this crate validates and renders, it performs almost no I/O and makes
/// almost no decisions that can fail.
#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum ChangelogError {
    /// §CL.4 — a `ChangelogEntry` was constructed with `severity: Severity::None`. A bug in
    /// the caller (`callisto-graph`'s aggregation), surfaced here rather than silently
    /// dropped, per §CL.3's doc comment.
    #[error("changelog entry for `{package}` has severity `none`, which never contributes \
             changelog content (§6.1); this entry should not have been constructed")]
    NoneSeverityEntry { package: PackageId },

    /// `input.entries` is empty — a package that was bumped must have at least one
    /// contributing entry (§CL.3's invariant); an empty list reaching `render_section` means
    /// something upstream computed a bump with no attributable cause, which is itself a bug
    /// worth surfacing rather than rendering a heading with nothing under it.
    #[error("changelog input for `{package}` has no contributing entries; a bumped package \
             always has at least one")]
    EmptyInput { package: PackageId },

    #[error("failed to read `{path}`: {message}")]
    Read { path: PathBuf, message: String },

    #[error("failed to write `{path}`: {message}")]
    Write { path: PathBuf, message: String },
}
```

Note what is **not** here: nothing resembling `ManifestError::FormattingNotPreserved`. There is
no existing structure to fail to preserve (§CL.6) — the only failure modes are "the input was
malformed" and "the filesystem said no." `callisto-graph` wraps this transparently
(`GraphError::Changelog`, §M.13.3).

### CL.9 Fixture obligations

1. **One golden `CHANGELOG.md` prepend per `ChangeSource` variant** — a before/after pair
   proving the exact heading/bullet shape in §CL.5's table, including the
   multi-`DependencyUpdate` nesting case and the `GroupUnion`/`NewGroupMember`
   no-authored-content case, and the file-does-not-exist-yet case that creates the
   `# <display_name>` header.
2. **A mixed-severity fixture** — one package bumped by a major changeset, a patch cascade,
   *and* a peer escalation in the same run, asserting section order (Major, then Patch — no
   Minor heading since nothing landed there) and that `PeerEscalation` renders standalone while
   `DependencyUpdate` entries nest under one shared parent bullet.
3. **The `Severity::None` rejection fixture** — constructing a `ChangelogEntry` with
   `severity: None` and asserting `ChangelogError::NoneSeverityEntry`, proving §6.1's no-op
   rule is enforced structurally, not just documented.
4. **A `compose-pr-body` preview vs. `version`-time render fixture** — the same
   `ChangelogInput` rendered through both call sites, asserting the output is **byte-identical
   including the heading**, since both now carry the same `to: Some(v)` (§G.11's
   `compose_pr_body` computes it via `plan_version`, §CL.7.2). Plus one `to: None` fixture for
   the residual all-`none`-severity case, asserting `## Unreleased`. This is the fixture that
   would catch the two call sites drifting apart, which §CL.2's "one render function, three
   consumers" decision exists to prevent — and the one that would have caught a preview mode
   that omitted versions from §12.2's `<summary>package@version</summary>` line.
5. **`extract_section` round-trip** (§CL.6, §CL.7.1) — for each §CL.9 item 1 golden: render,
   `prepend` into a `CHANGELOG.md` that already has two earlier sections, then
   `extract_section(&contents, &version)` and assert it returns the rendered section verbatim.
   Plus the `None` cases: a version with no matching heading, a heading that differs only by a
   `v` prefix, an empty section, and a missing file. This is the fixture that makes
   `ReleaseEntry.changelog_section`'s production path (§CL.7.1) real rather than asserted.
6. **`wasm32-wasip1` fixture run** — included in the same CI job as `callisto-model`'s (§13
   invariant 26), since this crate's file writes are exactly the kind of
   WASI-preopened-directory operation that job exists to prove works, not just compiles.

### CL.10 Index of `[SPEC DECISION]` flags

| # | Section | Decision |
|---|---|---|
| 1 | §CL.1 | `ChangelogInput` is a distinct, richer type from `BumpReason` — every contributing cause, not the dominant one. |
| 2 | §CL.2 | One render function, three consumers (`CHANGELOG.md`, `changelogSection`, PR body). |
| 3 | §CL.3 | `ChangeSource` has exactly six variants; `BumpReason::PreRelease` is deliberately not one. |
| 4 | §CL.5 | The exact bullet-text templates, as a design preference rather than a compatibility requirement. |
| 5 | §CL.6 | `Package.changelog: None` means opt-out, not "resolve a default path." |
| 6 | §CL.6/§CL.7.1 | `extract_section`'s deterministic read-back rule, and `ReleaseEntry.changelog_section` as an **optional** field produced by it — absent for a `changelog: None` package and for an unlocatable section. |

---

## 7. `callisto-graph`

**Purpose.** The coordination core: config resolution, project discovery, the dependency
graph, severity aggregation, the cascade fixpoint, groups, tags, the compute/apply split, and
one pure compute function per subcommand.

**License:** AGPL-3.0 (§16). **Milestone:** v0.1, with §G.14's module-level slicing.

This is the crate the decision doc's semantic-release analysis is about: *"The orchestration
loop must live in the core."* Aggregation (§7.1), graph construction (§7.2), cascade-to-fixpoint
(§7.4), group alignment (§7.5), and mutation ordering (§7.6) all live here, and no integration
— moon included — owns any part of that loop.

### G.1 Purpose, dependencies, and crate rules

#### G.1.1 License, milestone, API stability

`callisto-graph` answers four questions and owns the answers end to end:

1. **What packages exist?** — `ProjectLocator` + config resolution (§14, §7.2).
2. **What depends on what, with which spec string?** — `DependencyResolver` /
   `ManifestWalkResolver` (§7.2, §7.3).
3. **What version should each package become?** — aggregation (§7.1), cascade (§7.4), groups
   (§7.5).
4. **In what order may the result be published?** — `toposort` (§13 inv. 7, §9.2).

It does not parse changeset files (`callisto-format`), does not read or write manifest bytes
(`callisto-manifests`), does not parse commits (`callisto-conventional`), does not render
changelog text (`callisto-changelog`), does not parse argv or print (`callisto-cli`), and does
not know moon exists (§13 inv. 26, enforced in §G.1.7).

`callisto-graph`'s Rust API is **explicitly unstable pre-1.0**, documented as such in its
README (§15). The supported, versioned, fixtured contract is `--format json` on stdout
(§12.5, §0.1 rule 4). `DependencyResolver` and `ProjectLocator` in particular are expected to
take an Nx-style breaking interface change without that being an ecosystem event
(decision doc change 3).

**Types imported from `callisto-model`, declared nowhere here.** This crate declares **no**
value type that `callisto-model` already owns. Everything below is a `pub use` or a plain
import:

`Ecosystem`, `PackageId`, `GroupName`, `GroupKind`, `RegistryKey`, `ConfigKey`, `Version`, `VersionReq`,
`VersionGrammar`, `GrammarMismatch`, `Severity`, `Package`, `ManifestDecl`, `ManifestRole`,
`ManifestFormat`, `ReleaseTrigger`, `PublishTarget`, `DepKind`, `DepSpec`, `WorkspaceKind`,
`Coverage`, `DependencyEntry`, `DepEdge`, `ProjectRoot`, `DeclaredEdge`, `DeclaredEdgeKind`,
`TagTemplate`, `TagName`, `LastTag`, `LastTagSelection`, `select_last_tag`, `CommitSha`,
`CommandRunner`, `CommandOutput`, `CommandError`, `Diagnostic`, `DiagnosticCode`,
`DiagnosticSeverity`, `StrictFlag`, `SCHEMA_VERSION`, `Report`, and every §M.12 report type
(`PublishPlan`, `VersionReport`, `BumpRecord`, `BumpReason`, `StatusReport`, `SnapshotReport`,
`ComposePrBodyReport`, `ValidateReport`, `TagReport`, `InitReport`), `ModelError`,
`ManifestError`, `TagTemplateError`, `VersionParseError`.

Severity comparison throughout this section is written as a helper rather than as a bare
operator, because every cascade decision turns on **strict** raises and a non-strict comparison
would silently make the fixpoint's termination bound (§G.7.6) unprovable:

```rust
/// True iff `new` is strictly stronger than `cur` under §M.5's lattice
/// `None < Patch < Minor < Major`. §M.5's variant order is declared ascending precisely so
/// this is the derived `Ord`; the helper exists so every call site reads the same and a
/// future reordering breaks one line instead of thirty.
#[inline]
pub(crate) fn raises(new: Severity, cur: Severity) -> bool { new > cur }
```

#### G.1.2 Module layout

```
callisto-graph/
└── src/
    ├── lib.rs               # re-exports, Workspace, the compute/apply entry points (§G.11)
    ├── config/              # §G.5 — callisto.toml + moon.yml extension block
    │   ├── mod.rs           #   ResolvedConfig, CascadeConfig, ValidationConfig, Provenance
    │   ├── raw.rs           #   serde shapes of callisto.toml + moon.yml's extensions block
    │   ├── resolve.rs       #   raw → ResolvedConfig; set expansion, precedence, derivation
    │   └── groups.rs        #   GroupTable, GroupDef, GroupMember, two-pass validation
    ├── locate/              # §G.2
    │   ├── mod.rs           #   ProjectLocator trait (§15 verbatim), LocateError
    │   ├── root.rs          #   find_workspace_root
    │   ├── ignore_walk.rs   #   IgnoreWalkLocator
    │   └── git.rs           #   probe_git (§G.2.4)
    ├── identity.rs          # §G.4.2 — IdentityResolver, IdentityIndex (also used by
    │                        #          callisto-moon, §MO.4.3)
    ├── walk.rs              # §G.4 — ManifestWalkResolver::build
    ├── resolver.rs          # §G.3 — DependencyResolver trait, the concrete graph
    ├── toposort.rs          # §G.3.2 — Kahn + deterministic tie-break + cycle extraction
    ├── crosscheck.rs        # §G.4.6 — declared_edges comparison
    ├── aggregate.rs         # §G.6 — changeset load, severity aggregation, group unions
    ├── infer.rs             # §G.6.4 — SeverityInference, NoInference, CommitInference
    ├── cascade.rs           # §G.7 — cascade_action, the fixpoint, rewrite_spec
    ├── groups.rs            # §G.8 — alignment, target version, new-member exemption
    ├── napi.rs              # §G.8.4 — napi.targets drift cross-check, triple ⇄ role table
    ├── tags.rs              # §G.9 — last_tag_for, TagIndex
    ├── changed.rs           # §G.9.3 — changed_since_last_tag, §6.3 validation
    ├── plan.rs              # §G.10.1 — VersionPlan
    ├── apply.rs             # §G.10.2 — apply_version_plan
    ├── matrix.rs             # §G.11 `matrix` — manifest parsing, triple/role join, the
    │                        #   18-entry hostRunner/useCross table (own, not napi.rs's)
    ├── commands/            # §G.11 — one pure compute fn per subcommand
    │   ├── status.rs  version.rs  publish.rs  snapshot.rs
    │   └── validate.rs  tag.rs  pr_body.rs  init.rs  matrix.rs
    └── error.rs             # §G.12 — GraphError, ConfigError (LocateError in locate/)
```

#### G.1.3 What this crate requires from `callisto-manifests` and `callisto-format`

> `[SPEC DECISION, not in 00-design.md: the entry points this crate consumes from its two
> file-handling dependencies are enumerated here rather than discovered per call site.]` §15
> names the crates but not their surface. Naming the surface is what makes §G.1.7's dependency
> audit meaningful (an audit that cannot say what the allowed surface *is* cannot catch it
> growing) and what lets `callisto-fixtures` stub the boundary.

From `callisto-manifests`:

1. `OpenContext<'a>` and `open(&ManifestDecl, &OpenContext) -> Result<Box<dyn Manifest>, ManifestError>`
   (§CM.2).
2. `round_trip(Ecosystem, &DepSpec, &Version) -> Option<DepSpec>` (§CM.3) — the grammar half of
   spec rewriting; the policy half is §G.7.7's.
3. `WorkspaceCargoResolver` + `WorkspaceInheritance` + `WorkspaceInheritance::inherited`
   (§CM.4.4) — the read side is used at graph-construction time (§G.4.4), the resolver's two
   write methods at apply time (§G.10.2 steps 3 and 6).
4. `detect_npm_workspace_kind(&Path) -> Result<Option<WorkspaceKind>, ManifestError>` (§CM.5.4).

From `callisto-format`:

5. `parse_changeset(&str) -> Result<Changeset, ParseError>` (§F.5.2).
6. `bump_version(&Version, Severity) -> Result<Version, BumpError>` (§F.6.2).
7. `parse_pre_json`/`write_pre_json` and `PreState`/`PreMode` (§F.7).

From `callisto-changelog`: `ChangelogInput`/`ChangelogEntry`/`ChangeSource`,
`render_section`, `prepend`, `extract_section` (§CL.3, §CL.5, §CL.6).

From `callisto-conventional`, **only behind the `inference` feature** (§G.6.4):
`InferenceInput`, `InferenceWindow`, `PreMajorInferencePolicy`, `infer_severity`,
`resolve_pre_cursor`, `advance_pre_cursor`, `pre_cursor_ref_name`.

Spelled out, because a signature disagreement between two specs should be a spec conflict
rather than a merge conflict discovered at implementation time. **This block is a copy, and a
copy that disagrees with the original is worse than no copy** — every signature below is
reproduced verbatim from the section named in its trailing comment, and that section is
authoritative if the two ever diverge again:

```rust
// callisto-manifests
pub fn open(decl: &ManifestDecl, ctx: &OpenContext<'_>)
    -> Result<Box<dyn Manifest>, ManifestError>;                                 // §CM.2
pub fn round_trip(eco: Ecosystem, spec: &DepSpec, to: &Version) -> Option<DepSpec>;  // §CM.3

pub struct WorkspaceCargoResolver { /* §17 v0.1, §18 Q2 */ }                     // §CM.4.4
impl WorkspaceCargoResolver {
    pub fn load(root_manifest: &Path) -> Result<Self, ManifestError>;
    pub fn inheritance(&self) -> Result<WorkspaceInheritance, ManifestError>;
    pub fn write_version(&mut self, v: &Version) -> Result<(), ManifestError>;
    pub fn write_dependency(&mut self, name: &str, new: DepSpec)
        -> Result<(), ManifestError>;
}

pub struct WorkspaceInheritance {                                                // §CM.4.4
    pub root_manifest: PathBuf,
    pub version: Option<Version>,
    pub dependencies: BTreeMap<String, DepSpec>,
}
impl WorkspaceInheritance {
    /// The read side of §18 Q2, and the accessor §G.4.4 calls for an entry whose
    /// `DependencyEntry::inherited` is `true`. `None` when the root declares no
    /// `[workspace.dependencies]` entry of that name. Returns **no `DepKind`** — the root's
    /// table has no sections, so the caller keeps the member's own kind (§CM.4.4).
    pub fn inherited(&self, name: &str) -> Option<InheritedDep<'_>>;
}
pub struct InheritedDep<'a> { pub spec: &'a DepSpec, pub declared_in: &'a Path }  // §CM.4.4

pub fn detect_npm_workspace_kind(root: &Path)
    -> Result<Option<WorkspaceKind>, ManifestError>;                             // §CM.5.4

// callisto-format
pub fn parse_changeset(src: &str) -> Result<Changeset, ParseError>;              // §F.5.2
pub fn bump_version(v: &Version, sev: Severity) -> Result<Version, BumpError>;   // §F.6.2
pub trait Versioning: Send + Sync {                                              // §F.6.1
    fn grammar(&self) -> VersionGrammar;
    fn bump(&self, current: &Version, severity: Severity)
        -> Result<Version, BumpError>;                                           // §6.2
    fn bump_prerelease(&self, base: &Version, severity: Severity, tag: &str,
                       current: &Version) -> Result<Version, BumpError>;         // §8, §F.6.3
}
/// **`Option`, not a bare reference** — `None` for a declared-but-unimplemented grammar
/// (`Pep440`/`Maven`, §M.4.1). `bump_target` (§G.7.4) turns that into
/// `BumpError::UnsupportedGrammar` before any write happens.
pub fn versioning_for(grammar: VersionGrammar) -> Option<&'static dyn Versioning>;   // §F.6.1
pub fn parse_pre_json(src: &str) -> Result<PreState, PreJsonError>;              // §F.7
pub fn write_pre_json(state: &PreState) -> String;                               // §F.7

// callisto-changelog
/// Fallible: `ChangelogError::NoneSeverityEntry` and `::EmptyInput` are load-bearing
/// upstream-bug detectors (§CL.4, §CL.8), not decoration — an infallible signature here would
/// have to swallow both.
pub fn render_section(input: &ChangelogInput) -> Result<String, ChangelogError>;  // §CL.5
/// **Does file I/O.** Prepends to the package's `CHANGELOG.md` under `root`, creating it with
/// a `# <display_name>` header when absent. There is no pure string-splicing variant of this
/// function anywhere; `apply_version_plan` step 7 (§G.10.2) calls exactly this one.
pub fn prepend(root: &Path, changelog_path: &Path, display_name: &str, rendered: &str)
    -> Result<(), ChangelogError>;                                               // §CL.6
/// The `plan-publish` read-back path for `ReleaseEntry.changelog_section` (§CL.7.1).
pub fn extract_section<'a>(changelog: &'a str, version: &Version) -> Option<&'a str>;  // §CL.6
```

Note what is *absent* from the `callisto-format` list: there is no `load_changesets(&Path)` and
no `PreState::enter(&root)`. `callisto-format` is filesystem-free (§F.3), so directory reads,
filename sorting, and the `pre.json` read/write are this crate's (§G.6.1) and
`callisto-cli`'s (§CLI.6.4) respectively. `Versioning`/`versioning_for` is the grammar-dispatch
form §7.7 and §M.15 require; `bump_version` is the SemVer-path convenience over it, fallible
for the grammar precondition (§F.6.2, §11.2 R2). Everything this crate reaches for on the
pre-release path is on that trait too (`bump_prerelease`, §F.6.3) — there is no free-function
or graph-local second implementation of the `pre.N` counter.

#### G.1.4 Config resolution lives here, not in a tenth crate

> `[SPEC DECISION, not in 00-design.md: `callisto.toml` parsing and the resolved-config type
> live in `callisto-graph::config`, not in a separate crate.]` §14 specifies the config format
> and §15's crate list has no config crate. Config resolution is not independently reusable —
> `[[package-set]]` glob expansion needs the discovered project set, and group validation needs
> resolved `PackageId`s (§G.5.5's two-pass split exists for exactly that reason) — so a
> separate crate would either duplicate discovery or depend on `callisto-graph` and be
> consumed only by it. `ResolvedConfig` is `callisto-graph`'s, and `callisto-model` carries only
> `GroupName` and `ConfigKey` so that decisions can be *attributed* without depending on the
> parser (§M.15).

#### G.1.5 Dependencies

| Edge | Kind | Why |
|---|---|---|
| `callisto-graph → callisto-model` | normal | everything in §M |
| `callisto-graph → callisto-manifests` | normal | §G.1.3 items 1–4 |
| `callisto-graph → callisto-format` | normal | §G.1.3 items 5–7 |
| `callisto-graph → callisto-changelog` | normal | §7.6 step 7, and `compose-pr-body`'s preview |
| `callisto-graph → callisto-conventional` | **optional**, `inference` feature | §G.6.4 |
| `ignore` | normal | the non-moon project walk (§14, §G.2.2) |
| `toml`/`toml_edit`, `serde_yaml` (or equivalent) | normal | §14's two config surfaces |
| `globset` | normal | `[[package-set]] match` globs |
| `callisto-fixtures` | **dev**, `graph` feature | §G.15 |

**Deliberately absent:** `callisto-moon`, any moon crate, `callisto-cli`, and anything HTTP.

#### G.1.6 Feature flags

```toml
[features]
default = []
cargo = ["callisto-manifests/cargo"]
npm = ["callisto-manifests/npm"]
# v0.2 onward. Off at v0.1, where `infer::NoInference` is the only SeverityInference impl and
# `callisto-conventional` is not in the dependency tree at all (§17).
inference = ["dep:callisto-conventional"]
```

#### G.1.7 Zero-moon and invariant 27, enforced structurally

§13 invariant 26 and §0.1 rule 1 both require CI enforcement, and §0.1 rule 1 says
**transitively**. Three mechanisms were available; the choice is a **CI job** — a workspace
`xtask` that reads `cargo metadata`'s resolve graph — with `cargo-deny` bans as a cheap second
net.

**Why not feature flags.** A feature flag cannot express this rule, for two independent
reasons. First, features are additive and unified across a workspace build: if `callisto-cli`
enables a moon-shaped feature on any shared dependency, that feature is on for
`cargo test -p callisto-graph` in the same workspace too. Second and decisively — for a `moon`
feature to gate anything in `callisto-graph`, moon-aware code would have to *be* in
`callisto-graph`. There is nothing to gate; the boundary being defended is the absence of that
code.

**Why not a workspace lint.** `clippy.toml`'s `disallowed-types`/`disallowed-methods` and
`#![deny]` attributes operate on paths named in *this crate's own source*. They catch a direct
`use moon_config::…` and miss the actual failure mode — a moon crate arriving transitively
through an intermediate dependency, which is exactly what §0.1 rule 1 says the rule covers.

**Why the CI job works.** `cargo metadata --format-version 1` emits a fully-resolved
`resolve.nodes` graph, and a walk from a crate's node id over its `dependencies` edges is the
transitive closure, exactly, with no text parsing.

```
xtask dep-audit    # runs on every PR; no network beyond the normal cargo fetch

for crate in callisto-format callisto-model callisto-graph \
             callisto-manifests callisto-conventional callisto-changelog:
    closure = walk resolve.nodes from crate over dep_kinds {normal, build}
    assert closure ∩ FORBIDDEN == ∅

# The dev-edge closure is audited separately, and only for callisto-fixtures, because
# `cargo test -p callisto-graph` compiles dev-dependencies: a moon crate reachable through
# callisto-fixtures would break rule 2's wasmtime fixture run without breaking rule 1's
# normal-edge closure.
closure_dev = walk resolve.nodes from callisto-fixtures over dep_kinds {normal, build, dev}
assert closure_dev ∩ FORBIDDEN == ∅

FORBIDDEN = { callisto-moon } ∪ { p | p.name matches ^(moon|moonbase|moonutils|proto_|
                                                      warpgate|extism)(-|_|$) }
```

Rule 2's `wasm32-wasip1` fixture run under `wasmtime` (§0.1, §13 inv. 26) is a *consequence*
check, not the enforcement: moon's crates do not build for `wasip1`, so a violation usually also
fails there — but a moon crate that happened to be wasm-clean would slip through, which is why
the dependency audit is the primary mechanism and the wasmtime run is the behavioural one.

**Corollary — invariant 27, made structural rather than grep-based.** The same `xtask` asserts
that `callisto-cli`'s manifest does **not** depend on `callisto-manifests`. §13 invariant 27
("`callisto-cli/src` contains no graph-construction or cascade code") is otherwise enforced by
a grep, which is exactly the "discipline" P5 rejects. Forbidding the dependency makes it
impossible for `callisto-cli` to open a manifest at all, and therefore impossible for it to
build a graph. This has a load-bearing consequence for this crate's API shape:
**`ManifestWalkResolver::build` must open manifests itself** and must not accept a manifest
factory, handle, or `Box<dyn Manifest>` from its caller — otherwise the wrapper would need the
dependency back.

> `[SPEC DECISION, not in 00-design.md: §0.1 rule 1 and §13 invariant 26 are enforced by an
> `xtask dep-audit` CI job that walks `cargo metadata`'s **resolve graph**, not by feature
> flags and not by a lint.]` §13 invariant 26 says "CI-enforced" without naming a mechanism.
> `cargo metadata --no-dev-dependencies` gives the exact set of crates that would ship, for
> the exact feature combination CI builds — which is the thing the rule is about. A lint over
> `use` statements would miss a transitive edge, and a feature flag would make the rule
> opt-in-able. The audit is a workspace `xtask` so it can also carry §CLI.9 item 5 (the
> `callisto-cli → callisto-manifests` absence) and §MO.7's `octocrab`/`reqwest`/`tokio`
> absence, and so it can be self-tested (§G.15 item 12: an enforcement mechanism nobody has
> watched fail is not an enforcement mechanism).

> `[SPEC DECISION, not in 00-design.md: §13 invariant 27 gets a second, compile-time
> enforcement mechanism alongside the audit: `callisto-cli` must not depend on
> `callisto-manifests`, which forces `ManifestWalkResolver::build` to open manifests itself
> rather than accepting pre-opened handles from a caller.]` The invariant as written
> ("`callisto-cli/src` contains no graph-construction or cascade code") is a statement about
> source content, which is checkable but grep-shaped. Removing the dependency makes the whole
> class unwritable: a crate that cannot name `Manifest` cannot write a manifest. The cost is
> that `ManifestWalkResolver::build` owns `OpenContext` construction (§G.4.1) instead of
> receiving it — which is where it belongs anyway, since building it needs the discovered
> project set.

#### G.1.8 Path conventions and determinism

Every path stored in a model type is **workspace-root-relative and UTF-8** (§M.1.3). This crate
holds exactly one absolute path, `Workspace::root` (§G.11), obtained from `find_workspace_root`
(§G.2.1) or supplied by the caller, and joins it at every I/O call site. `CommandRunner::run`'s
`cwd` argument always receives `&Workspace::root` or a join of it.

Under moon's preopened-directory sandbox (§0.1 rule 2) an absolute host path is not addressable
at all. `LocateError::OutsideWorkspaceRoot` is therefore raised at the discovery boundary —
before a path is stored — rather than at the failing read, so the failure names the offending
project instead of the offending syscall.

Every collection that reaches an output value or a fixture is a `BTreeMap`/`BTreeSet`/sorted
`Vec`. `HashMap`/`HashSet` appear only where §15's signatures force them (`toposort`'s
`subset: &HashSet<PackageId>`), and are copied into a `BTreeSet` before iteration. This is not
style: the cascade fixpoint's *result* is order-independent (severity max is commutative and
associative), but its *attribution* (`BumpReason::Cascade { via, .. }`) is not, and §12.6
fixtures the rendered attribution.

### G.2 Workspace root and project discovery — `locate.rs`

The trait, verbatim from §15:

```rust
/// Project *discovery* only. §15, decision doc change 2, §18 Q1a/Q1b.
///
/// Split out of the deleted `MoonProjectGraphResolver` because moon answers exactly one of
/// the two questions well. moon's project graph is authoritative for *which projects exist*
/// (it sees `moon.yml`-declared projects and implicit edges a manifest walk cannot), and is
/// structurally incapable of answering *what version does this edge require* — moon's
/// `ProjectDependencyConfig` carries no version-requirement string at all, so it can never
/// supply what §7.4's cascade pivots on.
///
/// Implementations: `IgnoreWalkLocator` (here, §G.2.2), `MoonProjectLocator`
/// (`callisto-moon`, §MO.4). Selecting between them is the *wrapper's* job, not this crate's —
/// `callisto-graph` must not contain an "is moon available?" branch, or invariant 26 would be
/// a runtime property instead of a dependency-tree one.
pub trait ProjectLocator: Send + Sync {
    fn projects(&self) -> Result<Vec<ProjectRoot>, LocateError>;

    /// Non-authoritative cross-check only (§18 Q1b). moon's `DependencyScope` is deliberately
    /// **not** reused here — `DeclaredEdgeKind` is callisto-owned with an explicit, documented
    /// mapping applied by `MoonProjectLocator`, because the mapping is lossy in both
    /// directions (moon has `Root` and no `Optional`; moon's `Production` does not cleanly
    /// split into callisto's `Runtime` + `Optional`, which is precisely the distinction
    /// napi's `optionalDependencies` pattern depends on).
    ///
    /// The cross-check compares edge **presence** only, never kind equality (§G.4.6).
    fn declared_edges(&self) -> Option<Vec<DeclaredEdge>> { None }
}
```

`projects()` returns one `ProjectRoot` **per (root path, ecosystem)** — §M.8's decision. A Case
D directory therefore yields two `ProjectRoot`s at the same path; collapsing them into one
`Package` is graph construction's job (§G.4.3), not the locator's.

Ordering: implementations must return `ProjectRoot`s sorted by `(path, ecosystem)`, byte-wise on
the path. This crate re-sorts defensively, but the contract is on the trait so a fixture can
assert it of `MoonProjectLocator` without going through graph construction.

`LocateError` is declared in `locate/mod.rs`, verbatim as pinned by §M.13.3; that text is
authoritative for its variants and message strings and is not restated here.

#### G.2.1 `find_workspace_root`

```rust
/// Walks **up** from `start` looking for a workspace-root marker, returning the first
/// (nearest) ancestor that has one. §14, §18 Q5.5's `init` transcript.
///
/// The nearest-first rule matters for nested workspaces: a Cargo workspace vendored inside an
/// npm workspace is its own root, and treating the outer one as the root would put every
/// vendored crate into the release set.
///
/// Returns an absolute, canonicalized path — the one absolute path in the system (§M.1.3,
/// §G.1.8). `LocateError::WorkspaceRootNotFound` when no ancestor carries a marker.
pub fn find_workspace_root(start: &Path) -> Result<PathBuf, LocateError>;
```

Markers, in the order they are tested within a single directory. The order is irrelevant to the
*result* — presence of any one is sufficient — but is fixed so the diagnostic naming the marker
is deterministic:

1. `Cargo.toml` containing a `[workspace]` table.
2. `package.json` containing a `"workspaces"` key.
3. `pnpm-workspace.yaml`.
4. A `.moon/` directory.

#### G.2.2 `IgnoreWalkLocator`

```rust
/// The non-moon `ProjectLocator` (§14, §15). Uses the `ignore` crate so `.gitignore`,
/// `.ignore`, hidden-file rules, and global excludes are honoured — a `target/` or
/// `node_modules/` full of vendored manifests is the normal case, not the exception.
///
/// The root itself is included when it carries a package-bearing manifest: a single-crate
/// repo whose `Cargo.toml` has both `[workspace]` and `[package]` is one project at `.`
/// (§G.9.3 notes the consequence for change detection).
pub struct IgnoreWalkLocator {
    root: PathBuf,
    /// Extra directory names never descended into, on top of whatever ignore files say.
    /// Belt and braces: a workspace with no `.gitignore` still must not walk `target/`.
    skip: BTreeSet<&'static str>,   // "target", "node_modules", ".git", ".moon", "dist"
}

impl IgnoreWalkLocator {
    /// The root is baked in at construction, because `ProjectLocator::projects(&self)` takes
    /// no path argument (§15).
    pub fn new(root: &Path) -> Self;
    /// `find_workspace_root` then `new` — `callisto-cli`'s default construction (§CLI.4).
    pub fn discover(start: &Path) -> Result<Self, LocateError>;
}

impl ProjectLocator for IgnoreWalkLocator {
    fn projects(&self) -> Result<Vec<ProjectRoot>, LocateError>;
    /// Always `None` — the `ignore` walk has no independent declaration of edges to
    /// cross-check against, so §G.4.6's check is skipped entirely rather than compared against
    /// the manifest walk's own output (which would be tautological).
    fn declared_edges(&self) -> Option<Vec<DeclaredEdge>> { None }
}
```

**Algorithm.**

```
 1. Read the root's membership declarations, if any:
      cargo_members  ← Cargo.toml [workspace] members/exclude globs   (None if absent)
      npm_members    ← package.json "workspaces" globs, or pnpm-workspace.yaml `packages`
    Absent declarations mean "no membership filter for that ecosystem", not "no members" —
    a single-crate repo with `[package]` at the root and no `[workspace]` is legal.

 2. Walk `root` with `ignore::WalkBuilder`, `hidden(true)`, `git_ignore(true)`,
    `parents(false)`, filtering out any directory whose file name is in `skip`.

 3. For each directory `d` reached:
      if d/Cargo.toml parses and contains a `[package]` table   → candidate (d, Cargo)
      if d/package.json parses and contains a `"name"` field    → candidate (d, Npm)
    A Cargo.toml with `[workspace]` and no `[package]` is a *virtual* manifest: not a project.
    A package.json with no `"name"` is a config carrier (a workspace root, typically): not a
    project.

 4. Membership filter. A candidate (d, Cargo) survives iff cargo_members is None, or
    d matches a `members` glob and no `exclude` glob. Same shape for (d, Npm) against
    npm_members. This is what keeps an intentionally-excluded crate — a fuzz target, a
    scratch example — out of the release set even though its manifest is on disk.

 5. `d` is rewritten workspace-relative (§M.1.3); a `d` that does not start with `root` is
    `LocateError::OutsideWorkspaceRoot` (reachable through a symlink; refused at the boundary
    rather than at the read, because under moon's preopened-directory sandbox the read would
    fail with an unrelatable errno — §G.1.8).

 6. Emit `ProjectRoot { id, path: d, ecosystem }`, where `id` is the *provisional* identity:
    `PackageId::Bare(name_from_manifest)`. Promotion to `Prefixed` happens in graph
    construction (§G.4.3), which is the first point that can see whether a name collides
    across ecosystems — a locator looking at one manifest structurally cannot.

 7. Sort by (path, ecosystem). Return.
```

Walk errors (permission denied, broken symlink) become `LocateError::Walk { path, message }`
and abort. A workspace where discovery is silently partial is a workspace where a package
silently never gets versioned; P5 puts the failure at the walk, loudly.

Final `ProjectRoot::id` values — the promotion in step 6 — are produced by `IdentityResolver`
(§G.4.2), the same path `MoonProjectLocator` uses: §13 invariant 25's logic applied to package
identity, not just tag names.

#### G.2.3 `pnpm-workspace.yaml` as a fourth marker

> `[SPEC DECISION, not in 00-design.md: `pnpm-workspace.yaml` is a fourth workspace-root
> marker alongside §14's three.]` §14 and §18 Q5.5 name Cargo `[workspace]`, npm
> `"workspaces"`, and `.moon/`. A pnpm workspace declares its members in
> `pnpm-workspace.yaml` and frequently has a root `package.json` with **no** `"workspaces"`
> key at all, so the three-marker list would fail to find the root of a perfectly ordinary
> pnpm monorepo — and pnpm is a first-class committed target (§2.2's "npm (npm/pnpm/yarn
> workspaces)"). `LocateError::WorkspaceRootNotFound`'s message text in §M.13.3 lists all four.

#### G.2.4 `probe_git`

```rust
/// P7's runtime capability check, shared by both wrappers so neither re-derives it (§CLI.3,
/// §MO.5). Runs `git --version` once per invocation, parses the version, and returns
/// `CommandError::IncompatibleVersion` below the supported floor or
/// `CommandError::NotFound` when git is absent.
pub fn probe_git<R: CommandRunner>(runner: &R, cwd: &Path) -> Result<(), CommandError>;
```

### G.3 The graph — `resolver.rs`

#### G.3.1 `DependencyResolver`

```rust
/// The graph interface. §15, decision doc change 3.
///
/// Keeps `-> impl Iterator` and static dispatch deliberately: RPITIT is not dyn-compatible,
/// and boxing every method to buy dyn-compatibility would be paid for a third-party
/// implementor population that does not exist (§16's AGPL tier, decision doc's Option-B
/// critique). No boxing, no dyn, **no pre-1.0 stability promise**.
///
/// Two implementors, and the second is why the trait exists at all rather than
/// `ManifestWalkResolver` being a bare struct:
/// - `ManifestWalkResolver` (§G.4) — the real one, built by walking canonical manifests.
/// - `callisto-fixtures`' in-memory resolver (§CF.3) — constructs a graph from literal
///   `Package`/`DepEdge` values with no filesystem, which is what makes §7.4's cascade
///   table unit-testable as a pure function of a graph shape (§12.6, decision doc change 3).
pub trait DependencyResolver: Send + Sync {
    fn packages(&self) -> impl Iterator<Item = &Package>;
    fn dependencies_of(&self, id: &PackageId) -> impl Iterator<Item = &DepEdge>;
    fn dependents_of(&self, id: &PackageId) -> impl Iterator<Item = &DepEdge>;
    fn toposort(&self, subset: &HashSet<PackageId>) -> Result<Vec<PackageId>, GraphError>;
}
```

Contract notes that are not visible in the signatures and that both implementors must honour,
because the cascade fixpoint and every fixture depend on them:

- `packages()` yields in `PackageId` sort order.
- `dependencies_of` / `dependents_of` yield in `(other_endpoint, kind, from_manifest)` sort
  order. Both return an **empty** iterator for an unknown `id` rather than erroring — the
  cascade walks dependents of every bumped package, and a package with no dependents is the
  common case, not an error case.
- `dependents_of(x)` yields edges whose `to == x`; `dependencies_of(x)` yields edges whose
  `from == x`. Stated because `DepEdge`'s field names are directional and the two methods are
  otherwise trivially transposable by accident.
- One edge per **(declaring manifest, dependency entry)** — §M.7.3. A Case D package that
  depends on `@myorg/sdk` from its `package.json` and on `sdk` from its `Cargo.toml` produces
  two edges with the same `(from, to)` and different `from_manifest`. Cascade handles both;
  §G.7.3's rewrite de-duplication keys on `from_manifest`.

```rust
/// The real implementor: a graph built by walking every canonical manifest (§7.2). moon, when
/// present, contributes project *discovery* through `ProjectLocator` and a *cross-check*
/// through `declared_edges()`, never the edges themselves.
pub struct ManifestWalkResolver {
    packages: BTreeMap<PackageId, Package>,
    /// Sorted; indexed by the two maps below rather than scanned.
    edges: Vec<DepEdge>,
    /// Outgoing edge indices, keyed by `from`.
    out_index: BTreeMap<PackageId, Vec<usize>>,
    /// Incoming edge indices, keyed by `to`. The same edge values, indexed the other way —
    /// cascade walks dependents, so this is not a convenience, it is half the data structure.
    in_index: BTreeMap<PackageId, Vec<usize>>,
    /// Name → identity, for §5.4's resolution order. Kept after construction because
    /// changeset-entry resolution (§G.6.1) uses the identical index — §13 invariant 25's
    /// "one function" discipline applied to identity as well as to tag names.
    index: IdentityIndex,
    /// Emitted during construction, carried forward into every report envelope (§M.11.2).
    diagnostics: Vec<Diagnostic>,
}

impl ManifestWalkResolver {
    pub fn diagnostics(&self) -> &[Diagnostic];
    pub fn identity(&self) -> &IdentityIndex;
    pub fn get(&self, id: &PackageId) -> Option<&Package>;
}
```

#### G.3.2 `toposort`

```rust
/// Kahn's algorithm over the *publish-relevant* edge kinds, scoped to `subset`.
/// §13 invariant 7: "scoped to the intra-release set, not the whole workspace, and
/// `rustCrates[]`'s array order **is** that topological order."
fn toposort(&self, subset: &HashSet<PackageId>) -> Result<Vec<PackageId>, GraphError>;
```

```
 1. members ← subset, copied into a BTreeSet (determinism, §G.1.8).
    Any id in `subset` that `packages()` does not know → GraphError::UnknownPackage.

 2. Consider only edges with **both** endpoints in `members` and
    `kind ∈ {Runtime, Build, Optional}`.

 3. Kahn, with a deterministic tie-break: among the currently in-degree-zero nodes, always take
    the minimum by `PackageId` sort order. Without this, `rustCrates[]`'s array order — a
    fixtured contract per §13 inv. 7, consumed positionally by §9.3's worked example — would
    vary with hash iteration order.

 4. If the queue empties with nodes remaining, a cycle exists. Extract it in traversal order
    (DFS from the lowest-`PackageId` remaining node, following the residual subgraph, until a
    node repeats; return the repeated-node-to-repeated-node slice) and raise
    `GraphError::Cycle { cycle }`. `GraphError::Cycle`'s `Display` joins with " -> ", so the
    message names a walkable path rather than a set — an implementer chasing a cycle needs the
    path, and a set makes them re-derive it by hand.
```

> `[SPEC DECISION, not in 00-design.md: `toposort` considers only `Runtime | Build | Optional`
> edges and excludes `Dev | Peer`.]` §13 invariant 7 fixes the scoping (intra-release subset)
> and invariant 8 fixes the napi ordering, but neither names an edge-kind filter, and §9.2 only
> says the array order "is" the topological order. The sort exists for one purpose — publish
> order — and the question it answers is "which package must exist on the registry before
> which."
>
> - **Dev is excluded.** Dev-dependency cycles are legal and common in both Cargo and npm (two
>   crates that dev-depend on each other's test helpers). Including `Dev` edges would make
>   `toposort` fail on a large fraction of real workspaces for a reason that has nothing to do
>   with publish order — `cargo publish` does not need a dev-dependency to exist on the
>   registry first.
> - **Peer is excluded.** Peer dependencies are, by definition, resolved by the *consumer*, not
>   installed by the dependent, so they impose no publish ordering either.
> - **Optional is included.** napi main packages reference their platform packages through
>   `optionalDependencies` at exact versions, and §13 invariant 8 makes "platforms before their
>   main package" a hard ordering fact. Dropping `Optional` here would silently delete exactly
>   the ordering constraint the plan exists to encode.
>
> This filter is the smallest one that satisfies both invariants.

### G.4 Graph construction — `walk.rs`, `identity.rs`, `crosscheck.rs`

#### G.4.1 `ManifestWalkResolver::build`

```rust
/// Opens manifests itself, through `callisto_manifests::open` — it must **not** accept a
/// manifest factory, handle, or `Box<dyn Manifest>` from the caller, or `callisto-cli` would
/// need a `callisto-manifests` dependency and §13 invariant 27's structural enforcement
/// (§G.1.7) would collapse.
pub fn build<L: ProjectLocator, R: CommandRunner>(
    root: &Path,
    locator: &L,
    runner: &R,
    cfg: &ResolvedConfig,
) -> Result<ManifestWalkResolver, GraphError>;
```

```
 1. projects ← locator.projects()?                      (§M.8: one entry per (path, ecosystem))

 2. Build the OpenContext once (§CM.2, P2) — the reason this function, and not its caller,
    owns it (§G.1.7):
      workspace_root      ← root
      cargo_workspace     ← WorkspaceCargoResolver::load(root/"Cargo.toml")?.inheritance()?
                            when any Cargo.toml exists at the root, else None
      npm_workspace_kind  ← detect_npm_workspace_kind(root)?

 3. Case D collapse and identity promotion (§G.4.3): group `projects` by path, collapse
    same-path/different-ecosystem entries into one Package with one Canonical manifest each,
    and promote provisional `PackageId::Bare` values to `Prefixed` where §5.4 requires it.

 4. Config application (§14, §18 Q5.2), highest precedence first:
        moon.yml `extensions.callisto` block at the project root
      > `[[package]]` explicit entry
      > `[[package-set]]` glob match
      > derivation from the manifests on disk (§G.5.4's table)
      > callisto's built-in default
    Produces `Package`'s six §5.1 fields — `release_trigger`, `publish_to`, `changelog`,
    `tag_template` among them. A set matching nothing, or two sets claiming one package, is a
    `ConfigError` (§14, §G.5.4).

 5. Group resolution + validation (§G.5.5 pass 2). Runs here, before any severity exists,
    which is what "parse time (P5), not at mutation time" means operationally.

 6. Identity index build (§G.4.2's `IdentityIndex`).

 7. Edge extraction (§G.4.4): for each Package, for each Canonical manifest handle,
    `iter_dependencies()` → resolve each `DependencyEntry`'s `name` against the known
    `PackageId` set (§5.4's "check whether the name resolves to a known `PackageId` in the
    workspace *before* falling back to treating it as an opaque external dependency, never the
    reverse"). Unresolved names are external and are dropped, not recorded. Emit one `DepEdge`
    per (declaring manifest, resolved entry) — §M.7.3.

 8. Cross-check against locator.declared_edges() (§G.4.6), if any.

 9. napi detection (§G.4.5).
```

**What is *not* walked.** `Platform`-role manifests are not walked for edges and are not graph
nodes (§M.6.1). Their dependency entries are generated by `@napi-rs/cli` and are inherited
lockstep values, not independent declarations; treating them as edges would create N spurious
nodes per napi package and would make §13 invariant 20 ("platform manifests are never
independently tagged") a rule to remember rather than a shape.

`Lockfile`-role manifests are not walked at all. They are regenerated (§7.6 step 9), never read
for graph structure — a lockfile's resolved versions are an *output* of the specs, and reading
them as inputs would make the cascade react to its own previous run. (`callisto-manifests`
refuses a lockfile-role `open` outright, §CM.2, so this is structural as well as stated.)

#### G.4.2 Identity resolution — `IdentityResolver` and `IdentityIndex`

```rust
/// The single path from "a project root plus an ecosystem" to a `PackageId`. Used by
/// `ManifestWalkResolver::build` (step 3 above), by `IgnoreWalkLocator::projects`, and — the
/// reason it is `pub` at all — by `callisto-moon`'s `MoonProjectLocator` (§MO.4.3).
///
/// Constructed once per invocation because it owns the `OpenContext`-backing state (P2); a
/// per-call constructor would re-derive workspace-wide facts N times.
///
/// This is §13 invariant 25's reasoning applied to package identity rather than tag names: a
/// second, independently-evolved identity path in `callisto-moon` is precisely the shape of
/// `#2207`, just for `PackageId` instead of `TagName`.
pub struct IdentityResolver { /* workspace root + OpenContext-backing state */ }

impl IdentityResolver {
    pub fn new(workspace_root: &Path) -> Result<Self, GraphError>;
    /// Opens the canonical manifest at `project_root` for `ecosystem` and reads only
    /// `Manifest::package_name()` — no dependency parsing, no version read. Then applies
    /// §5.4's bare-vs-prefixed rule against the names seen so far.
    pub fn resolve(&self, project_root: &Path, ecosystem: Ecosystem)
        -> Result<PackageId, GraphError>;
}
```

Where `IdentityResolver` is the *per-project-root* entry point a locator needs, `IdentityIndex`
is the *workspace-wide* map that graph construction builds once discovery is complete and then
keeps. It is the single place a **name string** becomes a `PackageId`:

```rust
/// One index, three callers — changeset entries (§5.4, §G.6.1), dependency entries (§5.4's
/// "identity resolution for graph edges", §G.4.4), and group member names (§G.5.5) — for the
/// same reason §13 invariant 25 makes tag-name resolution one function: two independently-
/// evolved identity paths that disagree is release-please's `#2207`, and it is silent when it
/// happens.
pub struct IdentityIndex {
    bare: BTreeMap<String, PackageId>,
    prefixed: BTreeMap<(Ecosystem, String), PackageId>,
    /// Ecosystem-native package name → identity, per ecosystem. This is the map dependency
    /// entries resolve through: a `Cargo.toml` `[dependencies]` key is a crate name, not a
    /// `PackageId::display_name`, and for a prefixed identity the two differ.
    native: BTreeMap<(Ecosystem, String), PackageId>,
    /// `Platform`-role manifests, by their declared package name — group member resolution
    /// only (§G.5.5). Never a graph node (§M.6.1).
    platform: BTreeMap<String, (PackageId, PathBuf)>,
}

impl IdentityIndex {
    /// §5.4's resolution order for a name written by a *human* (a changeset entry, a group
    /// `members` list): exact bare → exact prefixed → implicit disambiguation from sibling
    /// entries in the same changeset → `GraphError::AmbiguousName { name, candidates }`.
    ///
    /// `siblings` supplies step 3: when `name` is ambiguous but exactly one candidate shares
    /// an ecosystem with an unambiguously-resolved sibling entry, that candidate wins.
    pub fn resolve_human(&self, name: &str, siblings: &[PackageId])
        -> Result<PackageId, GraphError>;

    /// Resolution for a name read off a *manifest*: scoped to the declaring manifest's own
    /// ecosystem, and **workspace-first** — §5.4(a) is explicit that the check is "does this
    /// name resolve to a known workspace package", never the reverse. An external registry
    /// package that happens to share a name with a workspace member must not be misread as
    /// an internal edge; scoping to the declaring ecosystem is what prevents a Cargo `serde`
    /// dependency from resolving to an npm workspace member called `serde`.
    pub fn resolve_native(&self, eco: Ecosystem, name: &str) -> Option<&PackageId>;

    /// The inverse of `native`: the **ecosystem-native** package name this identity was
    /// registered under in `eco`, or `None` when the package has no canonical manifest in
    /// that ecosystem.
    ///
    /// This is the only correct source for a name that an ecosystem's own tooling consumes —
    /// `CratePublish::name`, `NpmPublish::name`, `NpmMainPublish::name` (§M.12.2), and a
    /// dependency-table key in a `RewriteKey` (§G.7.3). `PackageId::name()` **cannot** serve
    /// that use: §G.4.3 explicitly blesses a Case D package whose `Cargo.toml` says `foo` and
    /// whose `package.json` says `@myorg/foo`, one `PackageId::Bare("foo")` with two divergent
    /// native names, and a single `name() -> &str` can only ever return one of them — handing
    /// `cargo publish -p` or `pnpm --filter` the other one's argument.
    pub fn native_name(&self, id: &PackageId, eco: Ecosystem) -> Option<&str>;

    /// Every `(ecosystem, native name)` this identity is registered under, in `Ecosystem`
    /// order. One entry for an ordinary package, two for Case D.
    pub fn native_names(&self, id: &PackageId) -> impl Iterator<Item = (Ecosystem, &str)>;

    /// §5.4's write rule, used by `callisto add` (rendered in `callisto-cli`) and by
    /// diagnostics: emit the shortest unambiguous form.
    pub fn display_form(&self, id: &PackageId) -> String;

    /// `Platform`-role manifests of `owner`, by declared package name and manifest path —
    /// the `platform` map above, filtered. The derivation source for
    /// `PublishPlan.npm_platform_packages[]` and `NpmMainPublish::depends_on_platforms`
    /// (§G.11), since a platform manifest is not a `Package` and therefore appears in no
    /// per-`Package` iteration (§M.6.1's SPEC DECISION M7).
    pub fn platforms_of(&self, owner: &PackageId) -> impl Iterator<Item = (&str, &Path)>;
}
```

> `[SPEC DECISION, not in 00-design.md: `callisto-graph` exposes `IdentityResolver` publicly
> so `MoonProjectLocator` can obtain `PackageId` values without `callisto-moon` reimplementing
> identity resolution.]` `ProjectLocator::projects()`/`declared_edges()` return
> `PackageId`-bearing values (§15) but take no arguments that could supply one, so *some*
> manifest read has to happen inside a locator. The two options were (a) `callisto-moon` opens
> manifests itself via `callisto-manifests`, or (b) it calls back into `callisto-graph`, which
> already performs this exact resolution once per manifest. (b) is smaller and keeps one
> resolution path. `identity.rs` is deliberately a dedicated module so it is easy to audit as
> *not* a second cascade or graph-construction entry point (§13 invariant 27's spirit applied
> one crate over). An earlier draft gave this a free-function signature taking a
> `callisto_manifests::OpenContext`; the struct form is the reconciliation, since it avoids
> making `callisto-moon` name a `callisto-manifests` type (§11).

#### G.4.3 Case D collapse and identity promotion

Group the locator's `ProjectRoot`s by path:

```
 - one root at a path                  → one Package, one Canonical manifest.
 - ≥2 roots at a path, distinct ecos.   → one Package, one Canonical manifest per root
                                          (§5.2's Case D; §18 Q5.2's "cheap default").
 - ≥2 roots at a path, same ecosystem   → impossible from a conforming locator; treated as
                                          GraphError::DuplicatePackage.
```

Then resolve identity, per collapsed project, from its per-ecosystem manifest names:

```
 - All manifests at one path agreeing on a bare name, and that bare name unique
   workspace-wide                                     → PackageId::Bare(name).
 - A bare name claimed by projects at two *different* paths in two different ecosystems
                                                      → both promote to
                                                        PackageId::Prefixed { ecosystem, name }
                                                        (§5.4).
 - A bare name claimed by two projects at different paths in the *same* ecosystem
                                                      → GraphError::DuplicatePackage { id, paths }.
 - A single project whose two canonical manifests resolve to different identities after the
   above                                              → GraphError::SplitIdentity { path, ids }.
```

`SplitIdentity` is the one thing §5.4(b) says must never happen silently: the single-`Package`,
aggregate-by-max model is what stops the two sides of a dual-published package from quietly
drifting apart, and a split identity reintroduces exactly that. The triggering shape is
`Cargo.toml` saying `foo` and `package.json` saying `@myorg/bar` where **both** names are
separately claimed elsewhere in the workspace.

Note the benign case that is *not* `SplitIdentity`: `Cargo.toml` says `foo` and `package.json`
says `@myorg/foo`, with neither name claimed elsewhere. Those are two ecosystem-native names for
one identity; the `Package`'s `PackageId` is the shortest unambiguous form (§5.4) and both
native names are registered in the identity index (§G.4.2) so a dependency entry naming either
resolves here.

#### G.4.4 Edge extraction, Cargo workspace inheritance, and rewrite de-duplication

```
for pkg in packages (BTreeMap order):
  for decl in pkg.manifests where decl.role == Canonical:
      m ← callisto_manifests::open(decl, &ctx)?
      for entry in m.iter_dependencies():        # DependencyEntry {name, kind, spec, inherited}

          # (a) Cargo workspace inheritance (§18 Q2, §17 v0.1).
          #     `foo.workspace = true` carries no value at the member; the value — and the
          #     rewrite target — is the root's [workspace.dependencies] entry. The member's
          #     `DependencyEntry` already carries the *resolved* spec (§CM.4.2) and the flag
          #     that says where it came from (§M.7.3); this step recovers the declaring FILE.
          #
          #     `entry.kind` is NEVER touched here. It is the member's own section's kind, and
          #     the root's `[workspace.dependencies]` table has no section to override it with
          #     (§CM.4.4) — so a `[dev-dependencies] foo.workspace = true` still yields a
          #     Dev-kind edge and still takes §7.4's Dev row.
          let (spec, declaring_path) =
              if entry.inherited {
                  let inh = cargo_ws.as_ref()
                      .and_then(|ws| ws.inherited(&entry.name))
                      .ok_or(ManifestError::MissingField {
                          path: cargo_ws_root.clone(), field: "workspace.dependencies" })?;
                  (inh.spec.clone(), inh.declared_in.to_path_buf())
              } else {
                  (entry.spec.clone(), decl.path.clone())
              };

          # (b) Resolve to a workspace identity, or drop the entry as external.
          let Some(to) = index.resolve_native(decl.ecosystem(), &entry.name) else { continue };

          # (c) Emit.
          edges.push(DepEdge {
              from: pkg.id.clone(), to: to.clone(),
              kind: entry.kind, spec,
              from_manifest: declaring_path,
              inherited: entry.inherited,
          });
```

Two consequences worth stating because they are easy to get wrong and expensive when wrong:

**`from_manifest` is the file that *declares the value*, not the file that names the
dependency.** For an inherited Cargo dependency that is the workspace root `Cargo.toml`. Ten
members inheriting `serde` produce ten edges with ten distinct `from` values and one shared
`from_manifest`. That is what makes §7.6 step 6's rewrite land in the root, once, instead of
writing ten member-local overrides that silently shadow the workspace value —
`ManifestError::WorkspaceInherited` exists (§M.13.2, §CM.4.4) precisely so a missed resolution
here fails loudly rather than producing that shadowing write.

**Rewrites therefore de-duplicate by `RewriteKey` (§G.7.3), and the de-duplication is safe.** A
rewritten spec is a pure function of the original spec and the dependency's new version
(§G.7.7); two edges sharing a manifest entry share both inputs, so they always compute the same
output. This is a lemma, not an assumption, and §G.15 item 6 fixtures it.

`RewriteKey` is `(target, name, kind)` where `target` is a *typed* `DepWriteTarget` — a member
manifest or a Cargo workspace-root dependency table — and `kind` is `None` for the latter,
because the root's table has no sections and one write there satisfies every inheriting member
whatever section each declared it under. Ten members inheriting `serde`, three of them under
`[dev-dependencies]`, therefore produce **one** `SpecRewrite`, not ten and not two. Without the
`None`, a workspace that inherits the same dependency under two different sections would emit
two rewrites of the identical value to the identical root entry — idempotent, but a redundant
write §CM.1.1's persist-on-every-call model would actually perform twice.

> `[SPEC DECISION, not in 00-design.md: `DepEdge::from_manifest` for a Cargo-inherited
> dependency (`foo.workspace = true`) is the **workspace root's** `Cargo.toml`, not the
> member's; and spec rewrites de-duplicate on `RewriteKey`, whose `kind` is `None` for a
> workspace-root target.]` §18 Q2 is
> explicit that "bumping a member's dep means editing the root, not the member." Recording the
> root as the declaring manifest at graph-construction time — rather than discovering it at
> write time via `ManifestError::WorkspaceInherited` and redirecting — means §7.6 step 6 has
> the right target from the start, and means three members inheriting one root dependency
> produce **one** `SpecRewrite`, not three redundant writes of the same value to the same
> file. `ManifestError::WorkspaceInherited` remains as the backstop for a missed resolution
> (§CM.4.4), not as the normal path.

#### G.4.5 napi detection at v0.1/v0.2 (§18 Q5.6 item 1)

Before v0.3, a `package.json` carrying a `napi` key produces
`DiagnosticCode::NapiCoordinationNotYetSupported` — severity `Warning`, message stating
explicitly that platform coordination ships in v0.3 and that until then only the main package
is versioned. This is emitted from graph construction so `init`, `status`, and `version` all
get it without three call sites remembering to ask.

#### G.4.6 The `declared_edges` cross-check (§7.2, decision doc change 2)

```rust
/// Compares moon's declared edges against the manifest-derived graph. **Presence only** —
/// kinds are never compared, because the `DependencyScope → DeclaredEdgeKind → DepKind`
/// mapping is lossy in both directions (§M.8, and §MO.4.4's table is where the lossiness is
/// written down). Warn by default; `--strict-graph` escalates; surfaced in `--format json` as
/// `DiagnosticCode::GraphEdgeDisagreement` so the Action/moon workflow can gate on it.
/// Takes no strictness flag: it runs inside `ManifestWalkResolver::build` (§G.4.1 step 8),
/// before any command function's options are in scope. Every diagnostic it emits carries
/// `escalated_by: Some(StrictFlag::StrictGraph)`, and the command boundary's `escalate`
/// (§G.11) promotes them when `--strict-graph` was passed — to `validate`, `version`, **or**
/// `status`, all three of which build a graph (§CLI.6.2, §CLI.6.3, §CLI.6.5).
pub fn crosscheck_declared_edges(
    graph: &ManifestWalkResolver,
    declared: &[DeclaredEdge],
) -> Vec<Diagnostic>;
```

```
 1. Filter `declared`:
      - drop edges whose `kind == DeclaredEdgeKind::Root`;
      - drop edges either of whose endpoints does not resolve to a known PackageId.

 2. built_pairs    ← { (e.from, e.to) : e ∈ graph.edges }   # kinds and manifests collapsed
    declared_pairs ← { (e.from, e.to) : e ∈ filtered }

 3. For each pair in declared_pairs \ built_pairs → one Diagnostic:
      code: GraphEdgeDisagreement, severity: Warning (promoted to Error by §G.11's `escalate`
      when --strict-graph was passed),
      escalated_by: Some(StrictFlag::StrictGraph),
      message: "moon declares <from> -> <to>{ (via <via>)} but no manifest declares it"
 4. For each pair in built_pairs \ declared_pairs → one Diagnostic, same code, mirrored text.
 5. Sort by (from, to, direction). Never compare `kind`.
```

Kinds are never compared because the `DependencyScope → DeclaredEdgeKind` mapping is lossy in
both directions (§15, decision doc change 2, §MO.4.4): moon has `Root` and no `Optional`, and
moon's `Production` does not split into callisto's `Runtime` + `Optional`. A kind comparison
would therefore fire on every `optionalDependencies` entry in every napi workspace — i.e. on
exactly the workspaces callisto exists for — and would train users to ignore the diagnostic.

> `[SPEC DECISION, not in 00-design.md: `DeclaredEdgeKind::Root` edges, and edges either of
> whose endpoints does not resolve to a workspace `Package`, are excluded from the cross-check
> entirely.]` §7.2 and the decision doc specify presence comparison without saying what the
> comparable set is. moon's `Root` scope describes an edge to or from the workspace root
> project, which has no analogue in callisto's per-package model at all (§MO.4.4) — comparing
> it would report a disagreement on every moon workspace, permanently. Likewise a moon project
> with no canonical manifest is not a callisto `Package` (§MO.4.2 step 3 skips it), so an edge
> touching one has nothing on callisto's side to agree or disagree with. The exclusion lives
> here, on the consuming side, rather than in `MoonProjectLocator`, so that a reviewer can find
> it: `MoonProjectLocator`'s mapping stays total and visibly reports `Root` edges (§MO.10 item
> 2 fixtures exactly that), and this function visibly drops them.

### G.5 Config — `config/`

#### G.5.1 Sources and precedence

Three sources, highest precedence first:

1. A project's `moon.yml` `extensions.callisto` block (§14, §G.5.2) — per-package overrides.
2. `callisto.toml`'s `[[package]]` entries (§14: "explicit `[[package]]` entries always win
   over set matches").
3. `callisto.toml`'s `[[package-set]]` entries, then the workspace-level tables
   (`[changesets]`, `[cascade]`, `[validation]`, `[registries.*]`).

Anything unset falls back to a built-in default, and `ResolvedConfig` records *which* of those
two happened per key (§G.5.4's provenance), because §18 Q5.2's `init` rule ("write a key only
when the workspace's answer differs from callisto's built-in default") and §13 invariant 28's
`(default)` marker (§CLI.5.3) both need exactly that fact.

```rust
/// Read `callisto.toml` (absent is legal — every key has a default, §18 Q5.5's L1 workspace).
/// Per-project `moon.yml` blocks are merged later, during graph construction (§G.4.1 step 4),
/// since they are keyed by project root and the project set does not exist yet here.
pub fn load(root: &Path) -> Result<ResolvedConfig, ConfigError>;
```

**Per-package resolution, in full.** Run during graph construction, once the project set is
known:

```
for each discovered package P (by project root path, workspace-relative):
    layers ← []
    if moon.yml block exists at P's root              → layers.push(MoonYml, block)
    if an explicit [[package]] whose `match` == P     → layers.push(CallistoToml, entry)
    if P's [[fixed-group]] sets any per-package key   → layers.push(CallistoToml, group)
    if P's [[linked-group]] sets any per-package key  → layers.push(CallistoToml, group)
    for each [[package-set]] whose `match` glob ⊇ P   → collect
        if more than one matches P                    → ConfigError::OverlappingPackageSets
        else                                          → layers.push(CallistoToml, set)
    layers.push(Default, derive_from_manifests(P))    # §18 Q5.2's derivation rules
    resolve field-by-field, first layer that specifies the field wins;
    record ConfigProvenance from the winning layer.

after all packages:
    for each [[package-set]] that matched nothing     → ConfigError::PackageSetMatchedNothing
    for each [[package]] that matched nothing         → ConfigError::PackageMatchedNothing
```

§14's "two sets claiming the same package is a hard config error" is `OverlappingPackageSets`;
an explicit `[[package]]` overlapping a set is **not** an error — §14 says explicit entries
"always win over set matches," which presupposes the overlap.

> `[SPEC DECISION, not in 00-design.md: `[[fixed-group]]`/`[[linked-group]]` blocks are a
> config layer, ranked between `[[package]]` and `[[package-set]]`.]` §14's
> `pre-major-inference` comment says the key is "available on `[[package-set]]`/`[[package]]`/
> `[[fixed-group]]`/`[[linked-group]]` blocks alike," and an earlier draft of this section
> silently dropped the two group layers — leaving a documented config surface with no
> resolution path, which would have failed as `ConfigError::UnknownKey` on a config §14 shows
> as valid. Ranking: a group is a hand-named membership list, so it is more specific than a
> glob set but less specific than an entry naming one package, and that is the order above.
> The fixed and linked layers cannot conflict with each other, because §14's parse-time
> disjointness check (§G.5.5) already rejects a package belonging to both. Only per-package
> keys are resolvable from a group block — at v0.1–v0.4 that is `pre-major-inference` alone;
> `publish-to`, `release-trigger`, `tag-template`, and `changelog` are per-package facts a
> group has no business asserting for its whole membership, and setting one there is
> `ConfigError::UnknownKey`.

> `[SPEC DECISION, not in 00-design.md: `moon.yml`'s `extensions.callisto` block also accepts
> `pre-major-inference` and `changelog`, beyond §14's four-key example.]` §14's YAML example
> shows `package-name`, `release-trigger`, `publish-to`, `tag-template` and is explicitly
> labelled a per-project override surface, not an exhaustive key list. Both additions are
> *per-package* keys that §14 already attaches to `[[package]]`, and `moon.yml` is the
> per-package layer for a moon workspace — a workspace that overrides `release-trigger`
> per-project but has to reach back into workspace-level `callisto.toml` to override
> `pre-major-inference` for the same project would be an arbitrary split, not a scope
> boundary. What `moon.yml` deliberately does *not* accept is anything workspace-level
> (`[cascade]`, `[validation]`, `[registries.*]`, group definitions): those are cross-package
> concerns, and a per-project file cannot coherently set them.

**Derivation** (`derive_from_manifests`, §18 Q5.2, so that `init` never has to write these):

| Field | Cargo source | npm source |
|---|---|---|
| `publish_to` | `["cratesIo"]`; `publish = false` → `[]`; `publish = ["alt"]` → that key | `["npm"]`; `"private": true` → `[]`; `publishConfig.registry` → that registry |
| `release_trigger` | `Changeset` | `Changeset` |
| `tag_template` | `None` (the §9.1 default applies) | `None` |
| `changelog` | `CHANGELOG.md` beside the manifest if present | same |

#### G.5.2 The `moon.yml` extension block

> `[SPEC DECISION, not in 00-design.md: `moon.yml`'s `extensions.callisto` block has the
> highest config precedence and is read by the core directly, as plain YAML, without any moon
> dependency.]` §14 shows the block and describes it as "per-project `moon.yml` extension
> block for package-scoped overrides" but does not rank it against `callisto.toml` or say who
> parses it. It must be read by the core, because `callisto-cli` (which never has moon
> available) must honour the same config a moon-hosted run does — §14's model is one config
> system, not two — and reading a five-key YAML mapping out of a file is not a moon dependency
> (§0.1 rule 1 is about moon crates, not about the existence of a file moon also reads).
> Highest precedence because it is the most specific scope: per-project beats per-set beats
> workspace-wide.

Keys, per §14: `package-name`, `release-trigger`, `publish-to`, `tag-template`, plus
`pre-major-inference` and `changelog`. Unknown keys under `extensions.callisto` are
**rejected**, not ignored (`ConfigError::UnknownKey`) — a typo'd override that silently does
nothing is the failure P5 exists to make structural.

#### G.5.3 `pre-major-inference`

```rust
/// Config surface for §14's `pre-major-inference` key. Modelled as two independently-gated
/// bools, not a single enum, because §7.1 describes `breaking → Minor` and `feat → Patch` as
/// "separately gated" — a workspace can plausibly want the breaking downgrade without the
/// feat downgrade (breaking-in-0.x is genuinely ambiguous pre-1.0; feat-in-0.x being "just a
/// patch" is a much more aggressive claim some teams will not want).
///
/// Defined here, not in `callisto-conventional`, even though it exists to configure
/// inference: `InferenceWindowSpec::policy` (§G.6.4) and `apply_pre_major` (§G.6.3) both
/// need this type **unconditionally**, including on `NoInference`'s path, which has no
/// `inference` feature and therefore no access to a `callisto-conventional` type at all — an
/// earlier draft defined it there and would not have compiled for that reason.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PreMajorInferencePolicy {
    /// `breaking → Severity::Minor`.
    pub breaking_to_minor: bool,
    /// `feat → Severity::Patch`.
    pub feat_to_patch: bool,
}

impl PreMajorInferencePolicy {
    /// Both transforms off — inference runs unremapped. This is `Default::default()` and is
    /// what every `Auto`-trigger package uses unless a workspace opts in (§7.1: "Default off").
    pub const OFF: Self = Self { breaking_to_minor: false, feat_to_patch: false };
}
```

> `[SPEC DECISION, not in 00-design.md: `pre-major-inference` is a three-valued string key —
> `"off"` (or omitted) / `"conservative"` / `"conservative-feat"`.]` §14 shows only
> `pre-major-inference = "conservative"`, while §7.1 describes the `breaking → Minor` and
> `feat → Patch` remaps as "separately gated." A single value cannot express two independent
> gates, and `PreMajorInferencePolicy` above has two bools precisely because §7.1 says they
> are separate. Three values is the smallest key shape that spans them:

| Value | `breaking_to_minor` | `feat_to_patch` |
|---|:--:|:--:|
| omitted / `"off"` / `false` | ✗ | ✗ |
| `"conservative"` | ✓ | ✗ |
| `"conservative-feat"` | ✓ | ✓ |

`"conservative"` keeps §14's literal spelling working with the semantics §7.1's first-listed
remap describes, which is what a user writing it from §14 would expect.

#### G.5.4 `ResolvedConfig`

```rust
#[derive(Clone, Debug)]
pub struct ResolvedConfig {
    /// Absolute. The one absolute path in this crate (§G.1.8).
    pub root: PathBuf,
    /// Workspace-relative. `.changeset` unless `[changesets] dir` says otherwise.
    pub changesets_dir: PathBuf,          // §14 [changesets].dir, default ".changeset"
    pub cascade: CascadeConfig,
    pub validation: ValidationConfig,
    pub registries: BTreeMap<RegistryKey, RegistryConfig>,
    /// Per-package resolved settings, keyed by identity. `Package`'s own fields (§M.6.1) are
    /// populated from this during graph construction.
    pub packages: BTreeMap<PackageId, PackageConfig>,
    pub groups: GroupTable,
    provenance: BTreeMap<ConfigKey, ConfigProvenance>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigProvenance { Default, Explicit }

impl ResolvedConfig {
    /// The lookup §CLI.5.3's `(default)` marker and §18 Q5.2's `init` write rule both need.
    pub fn provenance(&self, key: &ConfigKey) -> ConfigProvenance;
    /// The already-rendered value for `key`, for the attribution line. Returns the resolved
    /// value as a display string (`"patch"`, `"true"`, `"out-of-range"`), because the renderer
    /// prints it and nothing computes on it.
    pub fn rendered_value(&self, key: &ConfigKey) -> Option<String>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CascadeConfig {
    /// §7.4 / §14 — WHEN to cascade at all. Default `OutOfRange`.
    pub mode: CascadeMode,
    /// §7.4 / §14 — HOW HARD to bump once cascade fires. Default `Patch`.
    pub bump_severity: CascadeBumpSeverity,
    /// §13 inv. 9. `true` by default, opt-out not opt-in — the one non-inert default
    /// (§18 Q5.4).
    pub peer_escalation: bool,
    /// §7.3 / §13 inv. 15. `true` by default; §14 declines to recommend `false`.
    pub preserve_npm_ranges: bool,
}

/// §7.4's cascade-*trigger* axis: WHEN to cascade at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CascadeMode { OutOfRange, Always }

/// §7.4's cascade-*severity* axis: HOW HARD once cascade fires. Deliberately **not**
/// `Severity`: §14 admits only `patch | minor`, and a `major` cascade would make every
/// workspace's dependents unshippable on a routine dependency bump. Keeping the two axes as
/// two types is what stops them being conflated back into one four-valued key — an earlier
/// draft did exactly that, and the result cannot express `mode = "always"` with
/// `bump-severity = "minor"`, which §14 permits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CascadeBumpSeverity { Patch, Minor }
impl CascadeBumpSeverity { pub fn as_severity(self) -> Severity; }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ValidationConfig { pub allow_empty_changesets: bool }   // §6.3, default false

#[derive(Clone, Debug)]
pub struct RegistryConfig { pub kind: Ecosystem, pub url: Option<String> }

#[derive(Clone, Debug)]
pub struct PackageConfig {
    pub release_trigger: ReleaseTrigger,
    pub publish_to: Vec<PublishTarget>,
    pub tag_template: Option<TagTemplate>,
    pub changelog: Option<PathBuf>,
    pub pre_major_inference: PreMajorInferencePolicy,
}
```

Parse-time validation rejects `bump-severity = "major"` / `"none"` with
`ConfigError::InvalidBumpSeverity` naming the two legal values (§G.12), rather than admitting
them into the type and failing later.

`registries` is pre-populated with the two implicit entries §18 Q5.2 names (`cratesIo` → Cargo,
`npm` → Npm) **before** the file is read, so a workspace that declares neither still resolves
`PublishTarget::CratesIo`/`Npm` to a `RegistryKey` — and so `plan-publish`'s `publishTo`
(§M.12.2) is always a valid key string.

#### G.5.5 Groups, and the two-pass validation

> `[SPEC DECISION, not in 00-design.md: `GroupMember` is a two-variant type, and group
> validation splits into a syntactic pass (pre-discovery) and a resolution pass
> (post-discovery).]` §14 requires group validation "at parse time (P5), not at mutation
> time," but two of the three rules it states are not checkable before discovery: whether two
> group entries name the *same package* depends on `PackageId` resolution (`cargo/foo` and
> `npm/foo` are one package, §5.4), and whether a declared member exists on disk requires the
> walk. The syntactic pass catches what is wrong on the config's face — duplicate group names,
> empty groups, the same *literal string* in two groups — and runs before any I/O, so a
> config that is obviously wrong fails on its own terms. The resolution pass catches the
> identity-level cases and produces `GraphError::ConflictingGroupMembership` /
> `GraphError::MissingGroupMember`. Both are still "before mutation," which is what §14 and
> §13 invariant 13 actually require.

```rust
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GroupTable {
    fixed:  BTreeMap<GroupName, GroupDef>,
    linked: BTreeMap<GroupName, GroupDef>,
    /// Reverse indices. `raise` (§G.7.5) consults `fixed_of` on every strict severity raise,
    /// so it must be a lookup, not a scan.
    fixed_of:  BTreeMap<PackageId, GroupName>,
    linked_of: BTreeMap<PackageId, GroupName>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupDef {
    pub name: GroupName,
    pub kind: GroupKind,   // `callisto_model::GroupKind` (§M.2) — not redefined here
    /// Sorted. Mixed `Package` and `PlatformManifest` entries are normal for a napi group.
    pub members: Vec<GroupMember>,
}

/// §14's `members = [...]` list names two different kinds of thing, and conflating them is
/// what would break §13 invariant 20: a napi group's members are the main package (a real
/// `Package`, a release point) plus N platform *manifests* of that same package (§M.6.1),
/// which are not `Package`s and must never be tagged, aligned-checked, or bumped
/// independently.
///
/// §14's napi example is `members = ["@myorg/native", "@myorg/native-darwin-arm64", …]` — one
/// list in which the first entry names a package and the rest name manifests. Modelling that
/// as one enum keeps §14's config surface exactly as written while keeping the graph free of
/// N phantom nodes per napi package.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum GroupMember {
    Package(PackageId),
    PlatformManifest { owner: PackageId, role: ManifestRole, path: PathBuf, name: String },
}

/// The **selector** for `GroupDef::members`, and the discriminant of `GroupMember` above.
/// A separate, payload-free type is required rather than merely convenient: `GroupMember`'s
/// variants carry data (`GroupMember::PlatformManifest { owner, role, path, name }`), so
/// `GroupMember::Package` is not a value in Rust — it is a constructor taking a `PackageId` —
/// and `g.members(GroupMember::Package)` would not compile. Every call site in this document
/// that reads `g.members(Package)` / `group.members(PlatformManifest)` is the unqualified
/// spelling of `GroupMemberKind::{Package, PlatformManifest}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GroupMemberKind { Package, PlatformManifest }

impl GroupMember {
    pub fn kind(&self) -> GroupMemberKind;
}

impl GroupDef {
    /// Members of one kind, in `GroupMember`'s sort order.
    pub fn members(&self, kind: GroupMemberKind) -> impl Iterator<Item = &GroupMember>;
}

/// `[[fixed-group]]`/`[[linked-group]]` (§14), deserialized as-authored and not yet
/// identity-resolved: `members` are the raw strings from `callisto.toml` (a package name, or
/// — for a napi group — a mix of the main package's name and platform-manifest names), not
/// `PackageId`s or `GroupMember`s. `GroupTable::resolve` (pass 2, below) is what turns these
/// into real `GroupDef`s; `validate_syntactic` (pass 1) checks this raw shape before that
/// resolution needs to run at all.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct RawGroupTable {
    pub fixed: Vec<RawGroup>,
    pub linked: Vec<RawGroup>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RawGroup {
    pub name: GroupName,
    /// As written in `members = [...]`; unresolved strings, §14's mixed package/platform
    /// napi shape included.
    pub members: Vec<String>,
}

impl GroupTable {
    /// Pass 1 — **syntactic**, runs on the raw config before any discovery, so a config that
    /// is wrong on its face fails even in a workspace where discovery would fail first.
    /// Checks: no duplicate group names within a kind; no name string in two fixed groups;
    /// none in two linked groups; the two name-string sets disjoint; no empty `members`.
    /// Failures are `ConfigError` (§G.12).
    pub(crate) fn validate_syntactic(raw: &RawGroupTable) -> Result<(), ConfigError>;

    /// Pass 2 — **resolution**, runs immediately after graph construction's identity step
    /// (§G.4.1 step 5), still long before any severity exists. §14: "at parse time (P5), not
    /// at mutation time."
    pub(crate) fn resolve(
        raw: &RawGroupTable,
        index: &IdentityIndex,
    ) -> Result<GroupTable, GraphError>;

    pub fn fixed_group_of(&self, id: &PackageId) -> Option<&GroupDef>;
    pub fn linked_group_of(&self, id: &PackageId) -> Option<&GroupDef>;
    /// `Package` members only — the set the cascade's `raise` operator (§G.7.5) unions over.
    pub fn fixed_siblings(&self, id: &PackageId) -> impl Iterator<Item = &PackageId>;

    /// **Public**, unlike `resolve` above — for `callisto-fixtures` (§CF.3.2), which builds
    /// `GroupDef`s directly from literal test data and has no `RawGroupTable`/`IdentityIndex`
    /// to run the two-pass config pipeline against. Skips both passes' validation, since a
    /// hand-built fixture scenario is asserted correct by whoever wrote the test, not
    /// discovered from a workspace — callisto-graph's own callers always go through `resolve`
    /// instead, so this does not weaken §14's parse-time validation guarantee in practice.
    pub fn from_groups(fixed: Vec<GroupDef>, linked: Vec<GroupDef>) -> Self;
}
```

Resolution rules, per `members` entry:

1. `index.resolve_human(name, &[])` succeeds → `GroupMember::Package`.
2. Otherwise `index.platform.get(name)` → `GroupMember::PlatformManifest`.
3. Otherwise → `GraphError::MissingGroupMember { group, member }`. This covers §7.5's
   "`members` lists a platform whose manifest file is missing entirely — a hard error
   unconditionally": a config-declared required member that does not exist on disk is
   unresolvable, and unresolvable is a hard error, not a drift diagnostic (§G.8.4 relies on
   this).

Post-resolution checks — what pass 1 structurally cannot catch, namely two *different* name
strings resolving to one `PackageId`:

```
for id in all resolved Package members:
    if id ∈ two fixed groups            → GraphError::ConflictingGroupMembership { package, groups }
    if id ∈ two linked groups           → same
    if id ∈ a fixed and a linked group  → same
```

`ConflictingGroupMembership`'s message lists every conflicting group name, per §14's "reject
with a clear error listing the conflicting groups, rather than letting an ambiguous membership
surface as a confusing mutation-time alignment failure." The decision doc's group-priority
follow-on is explicit that rejecting is strictly better than arbitrating, and this is where the
rejection lives.

### G.6 Aggregation — `aggregate.rs`, `infer.rs`

#### G.6.1 Loading changesets

```rust
/// One changeset file, as this crate sees it: the parsed content plus the two pieces of
/// identity `callisto-format` deliberately does not carry (§F.5.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedChangeset {
    /// Workspace-root-relative path — `VersionPlan::consumed_changesets` and
    /// `Diagnostic::path` both want it.
    pub path: PathBuf,
    /// Filename without the `.md` extension — the id `pre.json`'s `changesets` array uses
    /// (§8) and `BumpReason::Changeset` reports.
    pub id: String,
    pub changeset: Changeset,
}

/// Reads `<root>/<cfg.changesets_dir>/*.md` in **filename-sorted order** (§6.1's
/// "Filenames arbitrary, sorted for deterministic read order"), skipping `README.md`,
/// `config.json`, and `pre.json`, and parses each via `callisto_format::parse_changeset`.
pub fn load_changesets(root: &Path, cfg: &ResolvedConfig)
    -> Result<Vec<LoadedChangeset>, GraphError>;
```

Each `Entry::name` is resolved to a `PackageId` by §5.4's order: exact bare match → exact
prefixed match → implicit disambiguation via sibling entries in the same changeset →
`GraphError::AmbiguousName` listing candidates.

#### G.6.2 Severity aggregation

Per-package severity is the max over every source (§7.1, `Severity`'s derived `Ord`, §M.5):
every changeset entry naming it (after `PackageId` resolution, so a Case D package's two
prefixed names aggregate into one), plus inference for `Auto`-trigger packages with no
changeset.

```rust
/// §7.1, start to finish: load → resolve → per-package max → inference → group unions.
/// Pure with respect to the filesystem except through `runner` (inference's commit window)
/// and `load_changesets`.
pub fn aggregate<D, R, I>(
    graph: &D,
    config: &ResolvedConfig,
    runner: &R,
    tags: &TagIndex,
    pre: Option<&PreState>,
    inference: &I,
) -> Result<Aggregation, GraphError>
where D: DependencyResolver, R: CommandRunner, I: SeverityInference;

/// The aggregation result. Feeds §G.7's fixpoint; nothing here is a version yet.
#[derive(Clone, Debug, Default)]
pub struct Aggregation {
    /// Severity of record per package, after inference and after the group unions.
    /// A package absent from this map has `Severity::None` and is not part of the release.
    pub severities: BTreeMap<PackageId, Severity>,
    /// Why, per package — the seed for `BumpRecord::reason` (§M.12.3).
    pub reasons: BTreeMap<PackageId, BumpReason>,
    /// How each package acquired a severity, **before** any group union. The linked-group
    /// joint-naming rule (§G.6.6) reads *only* this map, which is what makes "jointly named,
    /// not jointly cascaded to" a structural distinction rather than a comment.
    pub named_by: BTreeMap<PackageId, NamedBy>,
    /// Changeset files whose entries were all consumed. §7.6 step 8 deletes exactly these.
    pub consumed: Vec<PathBuf>,
    /// The `ChangelogInput` per package (§G.6.9).
    pub changelog_inputs: BTreeMap<PackageId, ChangelogInput>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NamedBy {
    /// A changeset entry named this package explicitly — including an explicit `none`.
    Changeset,
    /// `ReleaseTrigger::Auto` inference produced a severity from commits.
    Inference,
}
```

**The pipeline.**

```
 1. changesets ← load_changesets(root, cfg)?                            # §G.6.1, filename-sorted
    In pre-mode, drop any changeset whose `id` is already in `pre.changesets` (§8) — this is
    what keeps repeated `version` runs from re-incrementing untouched packages.

 2. Resolve entries. For each changeset, resolve every entry name through
    `IdentityIndex::resolve_human(name, siblings)` (§G.4.2), where `siblings` is the set of
    already-unambiguously-resolved identities from the *same* changeset (§5.4's implicit
    disambiguation). Unresolvable → GraphError::AmbiguousName / GraphError::UnknownPackage,
    listing candidates.

    §5.4's Case D collapse happens here for free: `"cargo/foo": patch` and `"npm/foo": minor`
    in one changeset resolve to the *same* `PackageId`, so step 3's max produces one `minor`
    written to both manifests — not two independently-versioned sides.

 3. severities[id] ← max over all entries resolving to id, across all changesets
    named_by[id]   ← NamedBy::Changeset
    reasons[id]    ← BumpReason::Changeset { changesets: sorted ids naming id }

 4. Inference (§7.1). For each package P with `release_trigger == Auto` and P ∉ severities:
      window ← InferenceWindowSpec {
          pathspecs: package_paths(P),                                  # §G.9.3
          since: pre-mode ? tags.pre_cursor(P) : last-tag sha for P,
          current_version, has_prior_release, policy: cfg.packages[P].pre_major_inference,
      }
      outcome ← inference.infer(P, window)?
      if outcome is Some and outcome.severity != Severity::None:
          severities[P] ← outcome.severity; named_by[P] ← NamedBy::Inference
          reasons[P]    ← BumpReason::Inference { commits: outcome.commit_count,
                                                  remapped: outcome.remapped }

    "A changeset always wins over inference" is the `P ∉ severities` guard — inference is not
    computed at all for a package a changeset named, so an explicit `major` cannot be remapped
    by `pre-major-inference` under any setting (§7.1, decision doc's 0.x-remap resolution).

 5. Fixed-group severity union — §7.1's pre-step. §G.6.5.

 6. Linked-group joint union — §7.5. §G.6.6.

 7. consumed ← every changeset all of whose entries resolved. (In pre-mode, `consumed` feeds
    `pre.changesets`, not deletion; outside pre-mode it feeds §7.6 step 8.)
```

#### G.6.3 Release triggers and the pre-major remap

`ReleaseTrigger::Changeset` packages take their severity from changesets only.
`ReleaseTrigger::Auto` packages take theirs from changesets when a changeset names them, and
from inference otherwise — §7.1's "a changeset always wins over inference," enforced here
rather than inside `callisto-conventional` (§C.7).

§7.1's opt-in pre-major remap is applied to the inferred severity, on this side of the seam:

```rust
/// §7.1 / the decision doc's 0.x-remap resolution, stated as a function so the rule has one
/// written form. Applied **only** to an inferred severity for an `Auto`-trigger package, never
/// to a changeset severity, and never inside `bump_version` (§6.2 stays rigid — no flag, no
/// config path reaches it, ever).
///
/// This crate — not `callisto-conventional` — owns the policy value: it reads it from config
/// (§G.5.3), hands it across the seam in `InferenceWindowSpec::policy`, and is the sole
/// production caller, inside `CommitInference::infer()` (§G.6.4), right after
/// `callisto_conventional::infer_severity` (§C.7) returns a **raw**, unremapped severity.
/// `callisto-conventional` deliberately does not apply this policy itself — bump-decision
/// policy is coordination logic (P6), which belongs here, not in the commit-classification
/// crate. `NoInference` never calls this function at all: it returns `Ok(None)` before any
/// severity exists to remap, so the only other caller is `callisto-fixtures`' direct unit
/// tests of this function as a pure transform.
///
/// Returns `(severity, remapped)`; `remapped` populates `BumpReason::Inference.remapped`,
/// which is what lets §18 Q5.4's attribution line say the remap fired.
pub fn apply_pre_major(
    inferred: Severity,
    policy: PreMajorInferencePolicy,
    current: &Version,
    has_prior_release: bool,
) -> (Severity, bool);
```

```
if policy is fully off                        → (inferred, false)
if current is not `0.y.z`                     → (inferred, false)
if current is `0.0.z`                         → (inferred, false)   # inert, and says so
if the package has no prior release tag       → (inferred, false)   # inert, and says so

match (policy, inferred):
    (breaking_to_minor, Major) → (Minor, true)
    (feat_to_patch,     Minor) → (Patch, true)
    (_,                 s    ) → (s, false)
```

The two inert cases are the whole reason this is specified rather than inlined: `0.0.z` and
no-prior-tag are exactly where release-please's `bumpMinorPreMajor` has leaked bugs for years
(its `#2087` and `#2635`), and P2's stateless detection makes "no tag yet" a *routine* state
rather than an edge case. When either fires and the policy is not off, `apply_pre_major` also
emits a `Diagnostic` naming `ConfigKey::PRE_MAJOR_INFERENCE` and saying the setting is inert for
this package and why. The code is `DiagnosticCode::PreMajorInferenceInert` (§M.11.2), severity
`Warning`, `governed_by: Some(ConfigKey::PRE_MAJOR_INFERENCE)`, no `escalated_by` — this is
§7.1's "no remap, tool says so explicitly," and §13 invariant 28's attribution rule applied to
a default that *didn't* fire, so there is no strictness level at which it should become a
failure.

> `[SPEC DECISION, not in 00-design.md: `apply_pre_major` is applied to the already-aggregated
> (max-of-all-commits) severity `callisto_conventional::infer_severity` (§C.7) returns, not
> per-commit before aggregation.]` §7.1's phrasing ("applied only when producing a severity
> for an `Auto`-trigger package") describes singular output, consistent with either order, so
> this is a genuine choice. The two orders are provably equivalent for this policy's specific
> transform table (`Major → Minor`, `Minor → Patch`, both monotonic and each keyed to a
> severity value that can only originate from one commit-type family — `Severity::Minor` can
> only come from a `feat` commit, `Severity::Major` only from a breaking one), so equivalence
> is not an assumption, it is a consequence of the transform table having no case where two
> different raw severities could collide into the same post-remap value from different
> sources. Aggregate-then-remap is simpler (one call site, not N) and makes `remapped`'s
> "changed the *inferred* severity" semantics a direct equality check on the aggregate rather
> than something reconstructed from per-commit deltas.

> `[SPEC DECISION, not in 00-design.md: the pre-major gate reads `Version::major()`/`minor()`
> and therefore only makes sense for `VersionGrammar::SemVer`.]` §7.1's "`0.y.z`" language is
> SemVer notation and both v0.1–v0.4's committed ecosystems (Cargo, npm) are SemVer-grammar
> (§7.7), so this is not a live gap today, but §M.4.1's `Version` is grammar-tagged precisely
> because a future ecosystem might not be. `apply_pre_major` is specified to be called only
> for SemVer-grammar `Version`s; a caller invoking it for a non-SemVer version is a caller
> bug, not something this function defends against by returning an `Err` — adding a fallible
> signature to every call site for a case that cannot occur under any committed milestone is
> the kind of speculative generality P4 warns against paying for before an ecosystem that
> needs it exists.

#### G.6.4 `SeverityInference` — the seam

```rust
/// The seam between aggregation and conventional-commit inference. Owned by this crate, not
/// by `callisto-conventional`, so that v0.1 can ship a working `version` with no inference
/// crate in the dependency tree at all (§17).
pub trait SeverityInference: Send + Sync {
    /// `Ok(None)` means "this package has no inferred severity" — the routine answer at v0.1
    /// and for any `ReleaseTrigger::Changeset` package. `git` is the caller's already-
    /// discovered `GitAccess` (`aggregate()`'s own parameter, itself `Workspace`-shared) — an
    /// impl needing commit history uses this rather than discovering its own, so an
    /// N-package workspace pays for one discovery, not N.
    fn infer(&self, pkg: &Package, git: &callisto_vcs::GitAccess<'_>,
              window: InferenceWindowSpec<'_>) -> Result<Option<InferenceOutcome>, GraphError>;
}

/// What the caller knows and the impl needs: pathspecs, the lower bound, and the pre-major
/// gate's two inputs. Mirrors `callisto-conventional`'s `InferenceInput` (§C.7) without this
/// crate's default build having to name that crate's types.
pub struct InferenceWindowSpec<'a> {
    pub pathspecs: &'a [PathBuf],
    pub since: Option<CommitSha>,
    pub current_version: &'a Version,
    pub has_prior_release: bool,
    pub policy: PreMajorInferencePolicy,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InferenceOutcome {
    pub severity: Severity,
    pub commit_count: usize,
    pub remapped: bool,
    /// `(sha, subject)` per commit, for `callisto-changelog`'s `ChangeSource::Commit`
    /// itemisation (§CL.3). Empty for `NoInference`.
    pub commits: Vec<(CommitSha, String)>,
}

/// v0.1's impl. Always `Ok(None)`. Not a placeholder to be deleted — it stays as the impl a
/// caller selects when `inference` is off, and as the one `callisto-fixtures` uses for
/// cascade tests that have no business running `git`.
pub struct NoInference;

/// v0.2's impl, behind the `inference` feature. A thin, **stateless** adapter over
/// `callisto_conventional::infer_severity` (§C.7) — it holds no fields of its own; the
/// caller's `GitAccess` is handed in per call via `infer`'s `git` parameter rather than
/// discovered here, so this type carries nothing to discover it with.
#[cfg(feature = "inference")]
pub struct CommitInference;

impl SeverityInference for CommitInference {
    fn infer(&self, pkg: &Package, git: &callisto_vcs::GitAccess<'_>,
              window: InferenceWindowSpec<'_>) -> Result<Option<InferenceOutcome>, GraphError>
    {
        let input = /* InferenceWindowSpec -> callisto_conventional::InferenceInput */;
        let raw = callisto_conventional::infer_severity(git, &input)?;
        if raw.commit_count == 0 { return Ok(None); }
        let (severity, remapped) =
            apply_pre_major(raw.severity, window.policy, window.current_version,
                             window.has_prior_release);   // §G.6.3 — the ONE call site
        Ok(Some(InferenceOutcome { severity, commit_count: raw.commit_count,
                                    remapped, commits: raw.commits }))
    }
}
```

As of this writing, `CommitInference` is never constructed anywhere in production — grep
confirms `callisto-cli` (§CLI.6.3.1) hardcodes `NoInference` unconditionally, regardless of
whether the `inference` feature is enabled, so v0.2's "wire `CommitInference` in behind the
feature flag" milestone commitment (§G.14) has not actually landed in the shipped binary
despite the library-level type existing and being unit-tested. `apply_pre_major`'s call site
inside `infer` (below) is real code, just not yet reachable from any command.

`callisto_conventional::infer_severity` (§C.7) returns a **raw**, unremapped severity — the
pre-major policy is deliberately not applied inside `callisto-conventional` at all, even
though an earlier draft of that crate's spec did so via a since-removed
`apply_pre_major_policy` function (§C.4). Applying policy to a bump decision is coordination
logic, and P6 puts coordination logic in `callisto-graph`, not in a crate whose job is commit
*classification*; duplicating the transform in both crates (which that earlier draft did,
verbatim) is the two-code-paths-for-one-concept shape invariant 25 warns against, just for
this policy instead of tag identity. `NoInference` never reaches `apply_pre_major` at all —
it returns `Ok(None)` unconditionally, before any severity exists to remap — so the function
has exactly one production call site (above) plus `callisto-fixtures`' direct unit tests of
it as a pure function; an earlier justification citing "the `NoInference` … path" for its
existence was describing a call that doesn't happen and has been corrected (§G.6.3).

> `[SPEC DECISION, not in 00-design.md: `SeverityInference` is a `callisto-graph`-owned seam,
> and the concrete `callisto-conventional` adapter (`CommitInference`) lives **in
> `callisto-graph`** behind an optional `inference` feature rather than in the wrappers.]`
> §17 defers inference to v0.2, so v0.1 must be able to build and ship `version` without
> `callisto-conventional`; a seam is the standard way to express that. The adapter's placement
> is the reconciliation of two drafts that disagreed: one said `callisto-graph` must not depend
> on `callisto-conventional` at all, the other had `callisto-cli` constructing a
> `callisto_conventional::CommitInference` that implements a `callisto-graph` trait — which no
> crate could actually write, since neither `callisto-conventional` (no graph dependency) nor
> `callisto-cli` (which would then be the only place the adapter exists, forcing
> `callisto-moon` to duplicate it) is a good home. An optional feature on `callisto-graph`
> keeps the v0.1 dependency tree clean, keeps both wrappers thin (§13 inv. 27, P6), and keeps
> exactly one adapter. §11 records it.

#### G.6.5 The fixed-group severity union pre-step (§7.1)

Before the cascade fixpoint runs, any severity assigned by changeset or inference to *any*
member of a fixed group is unioned by max across every `GroupMember::Package` in that group.
§7.1 is explicit that this is a single pre-step, not something interleaved with the fixpoint.
§G.7.5 records the one place that claim needed strengthening.

```rust
/// §7.1: "Before §7.4's cascade runs to fixpoint, any severity assigned (by changeset or
/// inference) to *any* member of a fixed group is unioned by max across every member."
pub(crate) fn union_fixed(agg: &mut Aggregation, groups: &GroupTable);
```

```
for g in groups.fixed:
    pkg_members ← g.members(Package)
    target ← max over { agg.severities.get(m) : m ∈ pkg_members }
    if target == Severity::None: continue
    for m in pkg_members where raises(target, agg.severities[m]):
        agg.severities[m] ← target
        agg.reasons.entry(m).or_insert(BumpReason::FixedGroupUnion { group: g.name.clone() })
        # named_by is deliberately NOT set: a union is not a naming event, and the
        # linked-group rule (§G.6.6) must not see one.
```

Note the union operates on `Package` members only. A napi fixed group has exactly **one**
`Package` member (its platform entries are `PlatformManifest`s of that same package, §M.6.1),
so for the napi case this function is a no-op and the lockstep is delivered by §7.6 step 4's
inherit-parent-version write instead. The union has teeth only for a user-declared
multi-package fixed group like §14's `members = ["foo", "foo-cli"]` — which is worth stating,
because it is what makes §7.1's claim that a single pre-step suffices tractable to reason about.

#### G.6.6 Linked-group joint detection (§7.5)

A linked group releases jointly when a changeset or inference assigns a severity to **≥2**
members in the same run. "Jointly releasing means jointly *named*, not jointly *cascaded to*"
— a cascade-induced bump landing on one linked member does not pull its siblings along.

```rust
/// §7.5: members share a version only when **jointly named**. Cascade never triggers this
/// (§G.7.5); that is the whole distinction, and it lives in the `named_by` filter below.
pub(crate) fn union_linked(agg: &mut Aggregation, groups: &GroupTable);
```

```
for g in groups.linked:
    named ← [ m ∈ g.members(Package) : agg.named_by.contains_key(m) ]
    if named.len() < 2: continue                    # not joint — independent version lines
    target ← max over { agg.severities[m] : m ∈ named }
    for m in named where raises(target, agg.severities[m]):
        agg.severities[m] ← target
        agg.reasons[m] ← BumpReason::LinkedGroupUnion { group: g.name.clone() }
```

Two points the design doc leaves implicit and this spec pins:

**Union scope is the named subset, not the whole group.** §7.5's wording is "a changeset or
inference assigning severity to ≥2 linked members … unions *their* severity by max." Unioning
across untouched members would pull a package into a release nothing named, which is the
fixed-group behaviour §7.5 defines linked groups *against*.

**An explicit `none` counts as a naming event.** `Severity::None` is a first-class changeset
severity (§6.1) and a human writing `"@myorg/docs": none` has named the package. It therefore
counts toward the ≥2 joint threshold, and the subsequent max union may raise it above `none` —
which is correct: the human's `none` recorded "this package changed but needs no bump of its
own," and joint release with a sibling that needs a minor is exactly the case linked groups
exist to express.

> `[SPEC DECISION, not in 00-design.md: an explicit `none`-severity changeset entry counts as
> a naming event for linked-group joint detection, and the union covers only the named
> subset.]` §7.5 says "a changeset or inference assigning severity to ≥2 linked members" without
> saying whether `none` counts as "assigning severity." It does: §6.1 makes `none` first-class
> and explicitly authored by a human ("a documented change with no version bump"), which is a
> statement of release intent, and intent is exactly what §7.5 says joint release is about. So
> a changeset naming `foo: none` and `foo-docs: minor` is joint, and both land at `minor`. The
> union covers the *named* subset only — a third linked member named by neither is untouched —
> because unioning across the whole group would make linked groups behave like fixed ones,
> which is the distinction §7.5 exists to draw.

#### G.6.7 A linked joint release forces a shared version, not just a shared severity

Severity union alone does not make linked members share a version: unlike fixed groups, their
base versions are not aligned, so `bump(1.4.0, minor)` and `bump(2.7.3, minor)` diverge. §7.5's
headline — "members share a version only when jointly releasing" — therefore needs one more
step, applied after target versions are first computed and **before** the cascade fixpoint
starts (§G.7.4):

```
for g in groups.linked with |named| ≥ 2:
    pairs  ← [ (m, targets[m]) : m ∈ named ]
    winner ← max over pairs' versions, by Version::compare
             # any pairwise compare() failure here is
             # → GraphError::GroupGrammarMismatch { group: g.name, members: pairs }
    for m in named: targets[m] ← winner
```

It is not re-applied inside the fixpoint, because §7.5 rules cascade out of joint intent.

> `[SPEC DECISION, not in 00-design.md: a linked joint release converges its named members on
> a shared maximum **version**, not merely a shared severity.]` §7.5 says linked members
> "share a version only when jointly releasing," which is a statement about versions; unioning
> severity alone would not produce one, because linked members are (by design) on independent
> version lines and applying the same severity to `1.2.0` and `2.4.1` yields two different
> versions. So: union the severity by max, apply `bump_version` to each member's own current
> version, then take the maximum result by `Version::compare` and assign it to every named
> member. This is what "share a version" has to mean for the sentence to be true, and it
> matches how §7.5's fixed groups already behave (identical severities plus identical aligned
> bases ⇒ identical results). §18 Q5.1 further records that `[[linked-group]]` is
> `@changesets/cli`'s `linked` feature under a different name, and this is exactly what
> `@changesets/cli` does — so the shared-version reading is also the P1-compatible one.

#### G.6.8 Pre-mode aggregation (§8)

When `pre.json` exists with `mode: "pre"`:

- Versions are computed from `initial_versions`, not from on-disk state, keeping
  `pre.0 → pre.1 → pre.2` monotonic (§8).
- Only changesets whose `id` is absent from `PreState::changesets` contribute; consumed ids
  are appended to it in `VersionPlan::pre_state_update`.
- `Auto`-trigger packages window from `refs/callisto/pre-cursor/<id>` (§C.6) rather than from
  the last stable tag, and the cursor is advanced in the same mutation phase (§G.10.2 step 8).
- **`initial_versions` is never an alignment-check input** (§8) — §G.8.2's signature makes
  that structural rather than remembered.

When `mode: "exit"`, the next `version` run compounds the full accumulated set into a real
non-prerelease version and then deletes `pre.json` (`VersionPlan::pre_state_update = None`
plus a deletion entry).

#### G.6.9 Building `ChangelogInput`

Aggregation is where every contributing cause is still in hand, so it is where
`callisto-changelog`'s richer input is assembled (§CL.1): one `ChangelogEntry` per contributing
changeset, per contributing commit, per cascade edge, and per group event, `Severity::None`
entries filtered out here (§CL.4), with the singular `BumpReason` for `VersionReport` derived
from the same data rather than the other way round.

### G.7 Cascade — `cascade.rs`

#### G.7.1 `cascade_action` — §7.4's table as a function

```rust
/// §7.4's cascade table, as one total function. Every row of the table is a match arm below,
/// and every arm carries the `ConfigKey` that governed it (§13 invariant 28) so the caller
/// never has to reconstruct the attribution from the outcome.
///
/// `source` is the severity of record for the *dependency's* move this run — the value that
/// lands in its `BumpRecord::severity`. It is what the "patch source" / "non-patch source"
/// distinction in the peer rows reads.
pub fn cascade_action(
    kind: DepKind,
    coverage: Coverage,
    source: Severity,
    cfg: &CascadeConfig,
) -> CascadeDecision {
    use Coverage::*;
    use DepKind::*;

    // §7.4's `mode` axis, and only that axis: `always` collapses **every** coverage answer
    // into the out-of-range branch for the **bump** decision — that is the literal content of
    // §7.4's "cascade every dependent regardless of range coverage", and the arm ordering
    // below is what makes the key non-inert. (An earlier draft consulted `cfg.mode` only for
    // the `Unknown` case, which left a covering spec under `mode = "always"` resolving to
    // `Covers` → `Severity::None` — i.e. the config key documented since §7.4/§14 as
    // "exposed in config from day one" did nothing at all on the rows that matter.)
    //
    // It never manufactures a rewrite — a spec that already covers the new version has
    // nothing to be rewritten to, and inventing one would violate §13 invariant 15's
    // leave-it-alone posture in the one direction the invariant did not anticipate.
    //
    // `Unknown` (§M.7.2: pnpm catalogs, `Opaque` multi-clause ranges) needs no rows of its
    // own: under `out-of-range` callisto cannot prove the spec broke, so it behaves as
    // `Covers` and warns; under `always` the first arm already settles it as `DoesNotCover`.
    // Either way the spec is never rewritten, which is what §13 inv. 15 actually requires.
    let effective = match (cfg.mode, coverage) {
        (CascadeMode::Always,     _)         => DoesNotCover,
        (CascadeMode::OutOfRange, Covers)    => Covers,
        (CascadeMode::OutOfRange, DoesNotCover) => DoesNotCover,
        (CascadeMode::OutOfRange, Unknown)   => Covers,
    };

    // Rewrite is gated on *real* coverage, never on `effective`.
    let rewrite = matches!(coverage, DoesNotCover);

    let (severity, governed_by, escalated) = match (kind, effective) {
        // ── row 1 ── Runtime / Optional / Build, spec covers ────────────────── none
        (Runtime | Optional | Build, Covers) => (Severity::None, None, false),

        // ── row 2 ── Runtime / Optional / Build, spec does not cover ───────────
        //             bump + spec rewrite; the severity is `bump-severity`'s value,
        //             not a constant (§14: `bump-severity` is HOW HARD, `mode` is WHETHER).
        (Runtime | Optional | Build, DoesNotCover) =>
            (cfg.bump_severity.as_severity(), Some(ConfigKey::CASCADE_BUMP_SEVERITY), false),

        // ── row 3 ── Peer, spec covers ──────────────────────────────────────── none
        (Peer, Covers) => (Severity::None, None, false),

        // ── row 5 ── Peer, genuinely out of range, non-patch source → MAJOR ────
        //             §13 invariant 9. Guarded on `coverage`, not `effective`: the invariant's
        //             own condition is "peer-dep **out-of-range** non-patch," so
        //             `mode = "always"` must not manufacture major bumps out of specs that
        //             actually cover. Ordered before row 4 so the guard is the discriminator.
        (Peer, DoesNotCover)
            if cfg.peer_escalation
               && matches!(coverage, DoesNotCover)
               && matches!(source, Severity::Minor | Severity::Major) =>
            (Severity::Major, Some(ConfigKey::CASCADE_PEER_ESCALATION), true),

        // ── row 4 ── Peer, out of range, patch source (or escalation opted out) ─
        (Peer, DoesNotCover) =>
            (cfg.bump_severity.as_severity(), Some(ConfigKey::CASCADE_BUMP_SEVERITY), false),

        // ── row 6 ── Dev, either way ─────────── Severity::None, rewrite only ──
        //             §13 invariant 10. Dev never bumps under either `mode`: `mode` decides
        //             *whether the coverage test gates the cascade*, and this row's outcome
        //             does not depend on the coverage test at all.
        (Dev, _) => (Severity::None, None, false),
    };

    // §13 invariant 28: when the bump happened *only* because `mode = "always"` overrode a
    // spec that genuinely still covers, `cascade.mode` is the key that caused it, and naming
    // `cascade.bump-severity` instead would send the user to the knob that decided how hard,
    // not the one that decided at all.
    let governed_by = match (cfg.mode, coverage, severity) {
        (CascadeMode::Always, Covers | Unknown, s) if s != Severity::None =>
            Some(ConfigKey::CASCADE_MODE),
        _ => governed_by,
    };

    CascadeDecision { severity, rewrite, governed_by, escalated,
                      unknown_coverage: matches!(coverage, Unknown) }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CascadeDecision {
    /// `Severity::None` means "no version bump" — row 6's whole point.
    pub severity: Severity,
    /// Rewrite this dependent's spec for this edge (§7.6 step 6).
    pub rewrite: bool,
    /// §13 invariant 28's attribution, computed here so `callisto-cli` renders rather than
    /// re-derives (P6, §18 Q5.6 item 2).
    pub governed_by: Option<ConfigKey>,
    /// Peer escalation fired → `BumpReason::PeerEscalation`, not `BumpReason::Cascade`.
    pub escalated: bool,
    /// Feeds a `RangeNotRoundTrippable` / `CatalogSpecNotRewritten` diagnostic.
    pub unknown_coverage: bool,
}
```

Read as §7.4's table:

| Edge kind | Coverage | `severity` | `rewrite` |
|---|---|---|---|
| `Runtime`/`Optional`/`Build` | `Covers` | `None` (`mode = always` → `bump_severity`, governed by `cascade.mode`) | ✗ |
| `Runtime`/`Optional`/`Build` | `DoesNotCover` | `bump_severity` | ✓ |
| `Peer` | `Covers` | `None` (`mode = always` → `bump_severity`, governed by `cascade.mode`) | ✗ |
| `Peer` | `DoesNotCover`, patch source | `bump_severity` | ✓ |
| `Peer` | `DoesNotCover`, non-patch source | `Major`, iff `cfg.peer_escalation` | ✓ |
| `Dev` | `Covers` | `None` | ✗ |
| `Dev` | `DoesNotCover` | `None` | ✓ |

> `[SPEC DECISION, not in 00-design.md: `mode = "always"` does **not** trigger peer
> escalation.]` §7.4's `always` says "cascade every dependent regardless of range coverage" and
> §13 invariant 9's condition is literally "peer-dep **out-of-range** non-patch." Reading
> `always` as also satisfying invariant 9's out-of-range condition would turn a knob documented
> as "forced downstream re-publishes" into a knob that produces unrequested major bumps across
> the workspace — a surprise §18 Q5.4 flags peer-escalation as *already* the sole sanctioned
> instance of. Keeping the guard on real coverage is the smaller reading and the more literal
> one. A covering peer edge under `always` therefore takes `cfg.bump_severity`, like every other
> covering edge.

> `[SPEC DECISION, not in 00-design.md: `Coverage::Unknown` behaves as `Covers` under
> `mode = "out-of-range"` and as `DoesNotCover` under `mode = "always"`, and is **never**
> rewritten in either case.]` §M.7.2 introduces `Unknown` for pnpm catalog specs and for
> `Opaque` multi-clause ranges; §7.4's table has no row for it, and §7.3/§13 inv. 15 fix only
> the never-rewritten half. Under `always` this falls out of the mode's own blanket rule
> (every coverage answer becomes `DoesNotCover` for the bump decision) rather than needing a
> rule of its own. Under `out-of-range`, cascading requires positive evidence that the
> spec stopped covering, and "cannot prove it broke" is not "it broke" — so the minimal-cascade
> default declines to bump and warns. Under `always`, the mode's own words settle the other
> side: coverage is not consulted at all and the dependent bumps. In both cases the spec itself
> is left alone with `DiagnosticCode::CatalogSpecNotRewritten` (or
> `RangeNotRoundTrippable` for a non-catalog `Unknown`), because callisto does not rewrite what
> it cannot read.

#### G.7.2 Coverage computation

```rust
/// Answers §7.4's "spec covers new version" column.
///   Exact(v)                → Covers iff v == new. npm's bare `"1.2.3"` is an exact-match
///                              requirement (§CM.5.2), and an exact pin never covers a
///                              different version.
///   CargoBare(v)            → **caret semantics** (§7.3, §M.7.1: "Cargo's bare `1.2.3`,
///                              which is semantically caret"): Covers iff `new` is caret-
///                              compatible with `v` — same non-zero leading component and
///                              `new >= v` — exactly as `Range` would answer for the
///                              equivalent `^v`. See `caret_covers` below.
///   Range(req, _)           → req.matches(new)?  → Covers | DoesNotCover
///   Workspace(_)            → Covers, always (§7.3: "never bumped, pnpm resolves")
///   Catalog(_)              → Unknown (§M.7.2)
///   Opaque(_)               → Unknown
pub fn coverage(spec: &DepSpec, new: &Version) -> Result<Coverage, GrammarMismatch>;

/// Cargo's caret rule, written once so `CargoBare` and a `^`-prefixed `Range` cannot answer
/// the same question two ways. `cur` is the declared version, `new` the candidate.
///
/// Covers iff `new >= cur` (by `Version::compare`) **and** `new` shares `cur`'s leading
/// non-zero component, where the leading non-zero component is:
///   - `major`, when `cur.major > 0`   → `^1.2.3` admits `1.9.0`, not `2.0.0`
///   - `minor`, when `cur.major == 0 && cur.minor > 0` → `^0.2.3` admits `0.2.9`, not `0.3.0`
///   - `patch`, when `cur` is `0.0.z`  → `^0.0.3` admits only `0.0.3`
/// A prerelease `new` additionally requires `cur` to be a prerelease of the same
/// `major.minor.patch`, matching Cargo's own requirement semantics.
pub(crate) fn caret_covers(cur: &Version, new: &Version) -> Result<bool, GrammarMismatch>;
```

This is the highest-blast-radius rule in the cascade: an ordinary `foo = "1.2.3"` Cargo
dependency is the single most common shape in a Cargo workspace, and treating it as an exact
pin would judge every in-range bump out of range — cascading a patch bump plus a spec rewrite
onto every dependent of every bumped crate, on every run. `Exact` and `CargoBare` are two
variants (§M.7.1) precisely because the identical literal string means a *different
requirement* in the two ecosystems; `coverage` is the function where that difference has to
actually show up.

#### G.7.3 The rewrite worklist

Every edge whose `CascadeDecision::rewrite` is `true` contributes a candidate `SpecRewrite`,
de-duplicated on `RewriteKey` per §G.4.4, and passed through `rewrite_spec`
(§G.7.7). The key is built by one function, so the inherited/not-inherited branch exists once:

```rust
impl RewriteKey {
    /// `eco` is the *declaring* manifest's ecosystem; `index` supplies the ecosystem-native
    /// dependency name (§G.4.2) — the key as it literally appears in the manifest's
    /// dependency table.
    fn of(edge: &DepEdge, eco: Ecosystem, index: &IdentityIndex) -> Result<Self, GraphError> {
        let name = index.native_name(&edge.to, eco)
            .ok_or(GraphError::UnknownPackage { id: edge.to.clone() })?
            .to_string();
        Ok(if edge.inherited {
            RewriteKey {
                target: DepWriteTarget::CargoWorkspaceDependency {
                    root_manifest: edge.from_manifest.clone(),
                },
                name,
                kind: None,
            }
        } else {
            RewriteKey {
                target: DepWriteTarget::Manifest(edge.from_manifest.clone()),
                name,
                kind: Some(edge.kind),
            }
        })
    }
}
```

`name` is never `PackageId::display_name`, which for a prefixed identity is a different string
and would name a dependency-table key that does not exist. `LeftAlone` outcomes emit `DiagnosticCode::RangeNotRoundTrippable` or
`DiagnosticCode::CatalogSpecNotRewritten` and produce no write.

```rust
/// Where a dependency-spec write actually lands, as a type rather than as a bare path the
/// apply step has to re-classify. §7.6 step 6 dispatches on this directly (§G.10.2).
///
/// Two targets need two different `callisto-manifests` entry points, and a bare `PathBuf`
/// cannot distinguish them — the workspace root's `Cargo.toml` is a legal value of both, since
/// a Cargo root may itself be a package (§G.2.2) *and* carry `[workspace.dependencies]`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DepWriteTarget {
    /// Edit this manifest's own dependency section, via
    /// `Manifest::update_dependency_spec(name, kind, new)` (§CM.1).
    Manifest(PathBuf),
    /// Edit `[workspace.dependencies].<name>` at this Cargo workspace root, via
    /// `WorkspaceCargoResolver::write_dependency(name, new)` (§CM.4.4) — which takes no
    /// `DepKind`, because the root's table has no sections.
    CargoWorkspaceDependency { root_manifest: PathBuf },
}

/// The de-duplication key. §G.4.4's lemma is what makes `or_insert` on this key sound: two
/// edges sharing a manifest entry share both of `rewrite_spec`'s inputs, so they always
/// compute the same output.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RewriteKey {
    pub target: DepWriteTarget,
    pub name: String,
    /// `Some(kind)` for a `Manifest` target — the section to edit. **`None` for
    /// `CargoWorkspaceDependency`**, so that members inheriting one root entry under
    /// different sections collapse to a single rewrite (§G.4.4).
    pub kind: Option<DepKind>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpecRewrite {
    /// Where to write, and what (§M.7.3, §G.4.4) — the workspace root for a Cargo-inherited
    /// dep, that dep's declaring member manifest otherwise.
    pub key: RewriteKey,
    /// Which workspace package this edge points at — carried so a diagnostic can name it
    /// without re-deriving it from `key.name`, which for a prefixed identity differs.
    pub dependency: PackageId,
    pub from: DepSpec,
    pub to: DepSpec,
}
```

#### G.7.4 The fixpoint — shape

```rust
/// §7.4's "runs to fixpoint," as a worklist over strict severity raises.
///
/// State: `severities` seeded by §G.6's aggregation, and `targets` derived from it by
/// `bump_target`. Worklist: a `BTreeSet<PackageId>` — ordered, so `pop_first` is
/// deterministic.
pub fn run_cascade<D: DependencyResolver>(input: CascadeInput<'_, D>)
    -> Result<CascadeOutcome, GraphError>;

pub struct CascadeInput<'a, D: DependencyResolver> {
    pub graph: &'a D,
    pub groups: &'a GroupTable,
    pub cfg: &'a CascadeConfig,
    /// Severities from §G.6, already unioned.
    pub seed: &'a BTreeMap<PackageId, Severity>,
    pub reasons: &'a BTreeMap<PackageId, BumpReason>,
    /// How each package acquired its seed severity (§G.6.2) — the linked-group joint-version
    /// union (§G.6.7) reads it, and nothing else in the fixpoint does.
    pub named_by: &'a BTreeMap<PackageId, NamedBy>,
    /// On-disk version of record per package (§M.6.1's canonical-manifest aggregate).
    pub base: &'a BTreeMap<PackageId, Version>,
    /// Pre-mode bump inputs (§8): `initialVersions`, the tag, and the current on-disk value
    /// the prerelease counter advances from. `None` outside pre-mode.
    pub pre: Option<&'a PreState>,
}

#[derive(Clone, Debug, Default)]
pub struct CascadeOutcome {
    pub severities: BTreeMap<PackageId, Severity>,
    /// The version each package converges on. Carried out of the fixpoint rather than
    /// recomputed, because §G.6.7's linked joint-version union has already overwritten some
    /// of them and re-deriving from severity would silently undo it.
    pub targets: BTreeMap<PackageId, Version>,
    pub reasons: BTreeMap<PackageId, BumpReason>,
    pub governed_by: BTreeMap<PackageId, ConfigKey>,
    /// Keyed by `RewriteKey` — §G.4.4's de-duplication lemma.
    pub rewrites: BTreeMap<RewriteKey, SpecRewrite>,
    pub diagnostics: Vec<Diagnostic>,
    /// For the fixture harness and for `CascadeNotConverged`'s message.
    pub iterations: usize,
}
```

```
targets  ← for each id in seed: bump_target(id, seed[id])        # below
apply the linked-group joint version union (§G.6.7)
worklist ← BTreeSet of every id in seed with a changed target

iterations ← 0
bound ← convergence_bound(graph.packages().count())              # §G.7.6

while let Some(pkg) = worklist.pop_first():                      # min PackageId — determinism
    iterations += 1
    if iterations > bound: return Err(GraphError::CascadeNotConverged { iterations })

    new_version ← targets[pkg]
    src_sev     ← severities[pkg]

    for edge in graph.dependents_of(pkg):                        # edge.to == pkg
        cov ← coverage(&edge.spec, &new_version)                 # §G.7.2
                .map_err(|source| GraphError::GrammarMismatch {
                    from: edge.from.clone(), to: edge.to.clone(), source })?
        d   ← cascade_action(edge.kind, cov, src_sev, cfg)

        if d.unknown_coverage:
            emit Diagnostic { code: match edge.spec {
                                  DepSpec::Catalog(_) => CatalogSpecNotRewritten,
                                  _                   => RangeNotRoundTrippable },
                              severity: Warning, package: Some(edge.from.clone()),
                              path: Some(edge.from_manifest.clone()),
                              governed_by: Some(ConfigKey::CASCADE_PRESERVE_NPM_RANGES), .. }

        if d.rewrite:
            match rewrite_spec(&edge.spec, &new_version, edge_ecosystem, cfg):
                Rewritten(to) => rewrites.entry(RewriteKey::of(edge, edge_ecosystem, index)?)
                                         .or_insert(SpecRewrite{..})
                LeftAlone(dg) => diagnostics.push(dg)             # §13 inv. 15

        if raises(d.severity, severities[edge.from]):
            raise(edge.from, d.severity, d, via = pkg,            # §G.7.5 — group-aware
                  edge = edge, dependency_to = new_version)
```

`raise` recomputes the affected package's target and re-inserts it (and, for a fixed-group
member, its siblings) into the worklist.

**`bump_target` — computing a version from a severity.**

```rust
/// The one function that turns a severity into a version, for every package, in every mode.
///
/// **Fixed-group members do not take the ordinary path.** If `id` belongs to a fixed group,
/// this delegates to `fixed_group_target` (§G.8.3) — memoised per `(GroupName, severity)` for
/// the run — and returns the group's target verbatim. Deriving each member's version from its
/// *own* base instead is correct only for an already-aligned group, and silently wrong for
/// exactly the case §7.5 and §13 invariant 22 exist to handle: a **new member**, whose own
/// base is whatever placeholder `@napi-rs/cli` scaffolded it with (`0.0.0`), so bumping from
/// it yields `0.0.1` rather than the group's target and leaves the group divergent on the
/// first run that includes it. Delegating makes §7.5's "unconditionally force-set to the
/// group's target version" fall out of the ordinary target computation rather than needing a
/// corrective write nobody calls.
///
/// Ordinary path, normal mode:
/// ```text
/// let versioning = callisto_format::versioning_for(grammar)
///     .ok_or(BumpError::UnsupportedGrammar { grammar })?;   // §F.6.1 — Option, not a panic
/// versioning.bump(base, sev)                                 // §6.2, verbatim, no remap,
/// ```                                                        // no config path reaching it (P1)
///
/// Ordinary path, pre-mode (§8):
/// ```text
/// versioning.bump_prerelease(initial_version, sev, &pre.tag, on_disk_version)   // §F.6.3
/// ```
/// — `base` is the package's `initialVersions` entry, **not** its on-disk version, so
/// `pre.0 → pre.1 → pre.2` stays monotonic across repeated runs; the *counter* advances from
/// the on-disk value, which is why `bump_prerelease` takes both. §8 is explicit that
/// `initialVersions` is a bump-computation input only and never an alignment-check input,
/// which is why `pre_mutation_checks` (§G.8.2) takes `base` and this function takes `pre`.
fn bump_target(id: &PackageId, sev: Severity, input: &CascadeInput<'_, impl DependencyResolver>)
    -> Result<Version, GraphError>;
```

A package absent from `initialVersions` in pre-mode is a package that joined the workspace
mid-cycle. §8 handles the fixed-group case (a synthesized entry equal to the group's current
entry, §G.8.3); a non-group package in this state gets an entry synthesized from its on-disk
version, which is the same rule with the group step removed.

#### G.7.5 `raise` — strictness, attribution, and the fixed-group union

```rust
/// Raises `id`'s severity to at least `sev`. Returns `true` iff this was a **strict** raise
/// (the stored severity actually increased, per `raises`, §G.1.1), which is the only thing
/// that pushes onto the worklist — that strictness is what makes §G.7.6's termination bound a
/// proof.
///
/// Fixed-group membership is consulted here; linked-group membership deliberately is **not**.
/// §7.5: "cascade is a mechanical consequence of a dependency edge, not an expression of
/// release intent, and 'jointly releasing' is about intent." A linked member moved by cascade
/// starts an independent version line; a fixed member cannot, because §7.5's fixed groups
/// "always share the exact version." The entire distinction is which table this function
/// consults, which makes it one line of code rather than a paragraph of discipline.
fn raise(
    pkg: &PackageId,
    sev: Severity,
    decision: &CascadeDecision,
    via: &PackageId,
    edge: &DepEdge,           // supplies `dep_kind` and `spec` for BumpReason's attribution
    dependency_to: &Version,  // `via`'s own new version — what actually triggered this raise
    out: &mut CascadeOutcome,
    groups: &GroupTable,
) -> bool;
```

```
out.severities[pkg] ← max(sev, out.severities[pkg])
out.reasons.entry(pkg).or_insert(
    if decision.escalated { BumpReason::PeerEscalation { via, spec: edge.spec.render() } }
    else                  { BumpReason::Cascade { via, dep_kind: edge.kind,
                                                   spec: edge.spec.render(),
                                                   dependency_to: dependency_to.clone() } })
out.governed_by.entry(pkg).or_insert(decision.governed_by)
recompute out.targets[pkg]; worklist.insert(pkg)

# Fixed-group invariant maintenance — NOT a second pass:
for sib in groups.fixed_siblings(pkg) where raises(sev, out.severities[sib]):
    out.severities[sib] ← sev
    out.reasons.entry(sib).or_insert(BumpReason::FixedGroupUnion { group })
    recompute out.targets[sib]; worklist.insert(sib)

# Linked groups: nothing. By construction.
```

`reasons` and `governed_by` use `or_insert`, never overwrite: the **first** raise that reached a
package's final severity is the attributed one, and because the worklist pops in `PackageId`
order, "first" is deterministic and fixturable. A diamond (`a → {b, c} → d`) therefore
attributes `d`'s bump to the lowest-`PackageId` predecessor rather than to whichever the map
happened to yield first.

> `[SPEC DECISION, not in 00-design.md: the fixed-group severity union is maintained by the
> fixpoint's `raise` operator, not only by §7.1's pre-step.]` §7.1 argues that a single
> pre-step suffices because a cascade's severity is capped at `cfg.bump_severity` and therefore
> cannot exceed a group's already-unioned target. That holds for §7.4's rows 2 and 4 — but not
> for row 5: peer escalation raises a single dependent to `Major` regardless of what the
> pre-step assigned, so a fixed group with one peer-escalated member and one un-escalated
> member ends the fixpoint divergent, and §7.6 step 2's alignment re-check — which §7.1 itself
> calls "a verification that this held" — would then hard-fail a run the user cannot fix by
> editing anything. Making the union a property of `raise` (a strict raise on any
> `GroupMember::Package` immediately raises its fixed-group siblings to the same severity)
> closes that hole without contradicting §7.1's words: it is not a second pass, and the
> fixpoint still converges in one pass, because the union is idempotent and monotone and every sibling
> raise is itself a strict severity raise already counted by the termination bound below.

#### G.7.6 Termination, and why the bound is a proof rather than a guess

`GraphError::CascadeNotConverged { iterations }` exists as P5's "turn a would-be hang into a
reportable bug." Its threshold is derived, not tuned:

- Every worklist insertion is caused by a **strict** severity raise on some package (`raise`
  is the only gate, in both the edge loop and the sibling loop).
- `Severity` is a four-element lattice of height 3 (`None < Patch < Minor < Major`), so each
  package can be strictly raised at most 3 times.
- The seed contributes at most `n` insertions.

Therefore at most `3n + n = 4n` pops occur in any terminating run, and

```rust
/// `4 * |packages| + 1`. Exceeding it is impossible for a correct implementation, which is
/// exactly what makes it a useful assertion: a trip means a bug in `raise`, in `Severity`'s
/// ordering, or in `bump_target`'s monotonicity — not a pathological workspace.
pub(crate) fn convergence_bound(package_count: usize) -> usize { 4 * package_count + 1 }
```

Result-order independence: severities are a max-fold, so the fixed point is unique regardless
of pop order. Only *attribution* is order-sensitive, and §G.7.5's `or_insert` + `pop_first`
pins it.

#### G.7.7 Spec rewriting (§7.3, §13 invariant 15)

```rust
pub enum RewriteOutcome {
    Rewritten(DepSpec),
    /// §13 invariant 15: leave the original string untouched and warn. Note this is an
    /// *outcome*, not an error — modelling it as `ManifestError` would make the default
    /// behaviour "abort the version pass," which is the opposite of what the invariant says
    /// (§M.13.2's closing note makes the same point from the error side).
    LeftAlone(Diagnostic),
}

/// Produce the new spec for one edge. A pure function of `(original, new_version, ecosystem,
/// cfg)` — which is what makes §G.4.4's rewrite de-duplication sound.
///
/// **The grammar work is `callisto-manifests::round_trip`'s** (§CM.3); this function is the
/// policy wrapper around it: the `preserve-npm-ranges` short-circuit, the round-trip
/// verification below, and the mapping from `None` to a specific `Diagnostic`.
pub fn rewrite_spec(
    original: &DepSpec,
    new: &Version,
    eco: Ecosystem,
    cfg: &CascadeConfig,
) -> RewriteOutcome;
```

Per-variant behaviour:

| `DepSpec` | Outcome |
|---|---|
| `Exact(_)` | `Rewritten(Exact(new))` |
| `CargoBare(_)` | `Rewritten(CargoBare(new))` — Cargo's bare `"1.2.3"` is semantically caret; the bare form is preserved, not normalised to `^1.2.3` |
| `Range(req, orig)` | operator- and precision-preserving re-render via `round_trip` (§CM.4.3/§CM.5.3), else `LeftAlone(RangeNotRoundTrippable)` |
| `Workspace(_)` | never reached — `coverage` is always `Covers` (§7.3: "never bumped, pnpm resolves") |
| `Catalog(_)` | never reached for rewrite; `coverage` is `Unknown` (§M.7.2), diagnostic `CatalogSpecNotRewritten` |
| `Opaque(_)` | `LeftAlone(RangeNotRoundTrippable)` |

`round_trip` recognises exactly these `Range` shapes, preserving both operator and
**precision** (`^1.2` → `^1.3`, not `^1.3.0` — a precision change is a diff a reviewer has to
read):

```
^X[.Y[.Z]]            → ^new at the same precision
~X[.Y[.Z]]            → ~new at the same precision
>=A <B                → >=new <next_major(new)      (only when B == next_major(A))
X | X.x | X.Y.x | *   → already covers; unreachable
```

Anything else — disjunctions (`^1 || ^2`), hyphen ranges, mixed comparators — is `LeftAlone`.

**The round-trip check is a check, not a claim.**

> `[SPEC DECISION, not in 00-design.md: round-trip fidelity is *verified* by re-parsing the
> candidate and re-checking coverage, not asserted by the renderer.]` §7.3's "if a bump cannot
> be confidently round-tripped" and §13 invariant 15 describe an outcome without saying how
> confidence is established. After producing a candidate string, `rewrite_spec` re-parses it
> and asserts both (a) it parses back to the same `DepSpec` discriminant with the same operator
> shape, and (b) `coverage(candidate, new) == Coverage::Covers`. Either failing yields
> `LeftAlone`. Without (b), a precision-preserving render like `^1.2` → `^2.0` reading as `^2`
> would be accepted while silently changing what the spec admits.

`cfg.preserve_npm_ranges == false` (§14's off-switch, which §14 itself declines to recommend)
short-circuits all of the above for `eco == Npm`: every `Range`/`Opaque` becomes
`Rewritten(Exact(new))`. §14 describes this as "always overwriting to an exact version,
knope-style"; it is implemented so the key is honest, and it emits no diagnostic, because
overwriting is what was asked for.


### G.8 Fixed and linked groups (§7.5) — `groups.rs`, `napi.rs`

#### G.8.1 Surface

```rust
/// §7.6 step 2's pre-mutation checks, as one call. Runs *before* any write (§13 inv. 13),
/// and after aggregation, so the group's target severity is known.
pub fn pre_mutation_checks<D: DependencyResolver>(
    graph: &D,
    groups: &GroupTable,
    base: &BTreeMap<PackageId, Version>,
    tags: &TagIndex,
    napi: &NapiTargetsIndex,
) -> Result<GroupCheckOutcome, GraphError>;
```

No `strict`/`strict_graph` parameter — per SPEC DECISION G35 (§G.11), every diagnostic this
function produces is emitted at `Warning` with `escalated_by: Some(StrictFlag::Strict)`
(alignment drift) or left at `Warning` with no escalation (there is no unescalatable case
here), and promotion to `Error` happens exactly once, centrally, via `escalate()` at the
command boundary — not by threading a bool through every check function that might want to
escalate something.

```rust

#[derive(Clone, Debug, Default)]
pub struct GroupCheckOutcome {
    /// Members exempt from the divergence check and force-set at write time (§13 inv. 22).
    pub new_members: BTreeMap<GroupName, Vec<PackageId>>,
    /// Warn-by-default drift (§7.5, §G.8.4). Escalated by `--strict`.
    pub diagnostics: Vec<Diagnostic>,
}

/// The raw `napi.targets` array from each fixed group's napi main package, read once
/// (`load`) rather than re-opened per group by `pre_mutation_checks` — the same
/// resolve-workspace-facts-once posture as `WorkspaceCargoResolver` (§CM.4.4), for the same
/// reason: §G.8.4's cross-check runs on every `version`/`status` invocation, and re-parsing
/// `package.json` per call site defeats the point of loading it once during graph
/// construction (§G.4).
#[derive(Clone, Debug, Default)]
pub struct NapiTargetsIndex {
    /// Keyed by fixed-group name. A group with no `"napi"` key in its main package's
    /// `package.json` is simply absent — not an empty `Vec` — so `pre_mutation_checks` can
    /// tell "no napi.targets to cross-check" apart from "declares zero targets."
    declared: BTreeMap<GroupName, Vec<String>>,
}

impl NapiTargetsIndex {
    /// Reads `napi.targets` from each fixed group's main package (`GroupMember::Package`,
    /// per §G.8.4's scoping) via `callisto-manifests`, for every group in `groups.fixed`.
    /// Called once, alongside the other graph-construction reads (§G.4), not per check.
    pub fn load(groups: &GroupTable, root: &Path) -> Result<Self, ManifestError>;

    /// `None` when the group's main package has no `"napi"` key at all — distinct from
    /// `Some(&[])`, which means the key exists but declares zero targets.
    pub fn declared_for(&self, group: &GroupName) -> Option<&[String]>;
}
```

`pre_mutation_checks` (§G.8.1) calls §G.8.4's `napi_drift` once per `g ∈ groups.fixed` where
`napi.declared_for(&g.name)` is `Some(declared)`, folding each call's diagnostics into
`GroupCheckOutcome::diagnostics` alongside the alignment check's own (§G.8.2/§G.8.3). A fixed
group with no napi manifest at all (an ordinary, non-napi fixed group — §7.5's mechanism is
not napi-exclusive) is simply skipped by this loop, not an error.

#### G.8.2 The fixed-group alignment check

```
for g in groups.fixed:
    released ← [ m ∈ g.members(Package) : tags.last_tag(m).is_some() ]
    fresh    ← [ m ∈ g.members(Package) : tags.last_tag(m).is_none() ]

    pairs ← [ (m, base[m]) : m ∈ released ]        # ON-DISK, in pre-mode exactly as in
                                                   # normal mode (§8, explicit)
    if any two of pairs' versions fail Version::compare (mixed grammars):
        return Err(GraphError::GroupGrammarMismatch { group: g.name, members: pairs })
    if pairs' versions contain ≥2 distinct values (by Version::compare):
        return Err(GraphError::FixedGroupDivergent { group: g.name, members: pairs })

    outcome.new_members[g.name] ← fresh          # §7.5's exemption, §13 inv. 22
```

Two scoping facts, both load-bearing:

**Only `GroupMember::Package` entries are checked.** `PlatformManifest` members cannot diverge:
§7.6 step 4 writes every platform manifest with the parent's version unconditionally, so a
platform manifest's on-disk value is an output, never an input. §7.5's new-member exemption is
explicitly justified by this ("inherit-parent-version already overwrites whatever placeholder
was there"), and scoping the check this way is what turns that justification into a structure
rather than a comment.

**`base` is on-disk, in pre-mode too.** §8 states this as "a deliberate, single answer to what
was previously an unaddressed ambiguity." The signature enforces it: `pre_mutation_checks` has
no `PreState` parameter at all, so `initialVersions` is not reachable from here.

#### G.8.3 The group target version, and new members

```rust
/// The version every member of a fixed group converges on this run.
///
/// **Called from `bump_target` (§G.7.4), for every fixed-group member, on every target
/// computation** — not from a separate forcing pass. That wiring is what makes §7.5's
/// new-member force-set real: the target is computed from the *group's* aligned base and the
/// *group's* severity, so a member whose own on-disk version is a scaffolding placeholder
/// gets the group's version rather than a bump of its placeholder. Memoised per
/// `(GroupName, severity)` per run; §7.1's severity union plus §G.7.5's `raise` guarantee
/// every `GroupMember::Package` in the group carries the same severity whenever this is
/// consulted, so the memo key is total and the result is the same for every member by
/// construction rather than by a subsequent alignment fix-up.
pub fn fixed_group_target(
    g: &GroupDef,
    base: &BTreeMap<PackageId, Version>,
    severities: &BTreeMap<PackageId, Severity>,
    tags: &TagIndex,
    pre: Option<&PreState>,
) -> Result<Version, GraphError>;
```

The group's severity is the max over its `GroupMember::Package` entries — identical to each
member's own severity in the ordinary case, and the max is what keeps the function total if a
future edit ever lets them differ momentarily.

The aligned base is the single distinct on-disk version among *released* `Package` members
(§G.8.2 has already proved there is exactly one). When a group has **no** released member — a
napi package landing for the first time, §Q5.5's L3 transition — there is no aligned base to
read, and this spec picks one:

```
released members exist            → the single aligned version
else, exactly one release-point member (`Package::is_release_point`, §M.6.1)
                                  → that member's on-disk version
else                              → pairs ← [ (m, base[m]) : m ∈ g.members(Package) ]
                                     max over pairs' versions by Version::compare
                                     # any pairwise compare() failure here is
                                     # → GraphError::GroupGrammarMismatch
                                     #     { group: g.name, members: pairs }
```

> `[SPEC DECISION, not in 00-design.md: the aligned-base fallback for a fixed group in which
> no member has a prior release tag.]` §7.5 specifies the new-member exemption for *individual*
> members joining an established group and never addresses a wholly-new group, which is the
> normal state of a napi package on the run that introduces it. Preferring the release-point
> member is consistent with §9.2/§13 inv. 20's model of which member is the real release
> point; the max fallback is the conservative choice when that is undecidable, since it never
> moves a member's version backwards.

New members (§7.5, §13 inv. 22) are force-set to this target on their first inclusion, with
`BumpReason::NewGroupMember { group }`, and never raise `FixedGroupDivergent`. The force-set is
not a separate corrective write: it is what `bump_target`'s delegation to this function
*already does* (§G.7.4), since the target is computed from the group's aligned base rather
than from the new member's own placeholder version. In pre-mode, at the same moment, an
`initialVersions` entry is synthesized for the new member equal to the group's current
`initialVersions` entry (§8) — carried in `VersionPlan::pre_state_update`, so it is written by
the same apply step that writes `pre.json`, not by a side effect here.

**How the exemption reaches a napi *platform manifest*, given that platform manifests are not
`PackageId`s (§M.6.1's SPEC DECISION M7).** This looks like a gap and is not one, but the
reason has to be written down, because §7.5's exemption is phrased in terms of "members with
no prior release tag" and a platform manifest can never appear in `GroupCheckOutcome::
new_members`, which is keyed by `PackageId`:

- A platform manifest is **never divergence-checked in the first place** (§G.8.2 scopes the
  check to `GroupMember::Package`), so there is no check for it to be exempted *from*. §7.5
  gives exactly this justification — "inherit-parent-version already overwrites whatever
  placeholder was there."
- A platform manifest is **always force-set**, not just on first inclusion: §7.6 step 4 writes
  every `Platform` manifest with the parent's new version unconditionally, on every run. A
  target added to `napi.targets` mid-cycle therefore lands on the group's version on the very
  first `version` run that sees it, with no new-member bookkeeping involved.
- §8's `initialVersions` synthesis likewise does not apply to it, and must not: `pre.json`'s
  `initialVersions` is keyed by *package* (§G.11's key rule), a platform manifest has no
  independent version line to have a baseline for, and its version is computed as its parent's
  regardless of what any baseline said. §8's napi-mid-pre-cycle scenario is therefore fully
  handled by the composition of these two rules — the *main package* keeps its existing
  `initialVersions` entry and its normal `pre.N` progression, and the new platform manifest
  inherits whatever that produced.

What genuinely remains for a v0.3 implementer is narrower than "the exemption is unreachable":
the `napi.targets` drift cross-check (§G.8.4) will warn every run until the new target is added
to `members` via `callisto init`, and until it *is* in `members` the manifest is not written at
all, because §7.6 step 4 iterates the group's configured `PlatformManifest` members — config is
authoritative (§13 inv. 21), so an undeclared platform is not silently versioned. That is the
intended, reviewable behaviour, not a hole; it is stated here so a v0.3 implementer does not
"fix" it by auto-adding members.

#### G.8.4 The `napi.targets` drift cross-check (§5.3, §7.5, §13 inv. 21)

```rust
/// `napi.targets` is read **every run**, purely as a cross-check against the group's
/// configured `members`, and **never** as a membership source. §13 invariant 21: membership
/// changes are only ever written via an explicit, reviewable `init`/sync flow.
///
/// The comparison is on **target triples**, not on derived package names.
pub fn napi_drift(
    group: &GroupDef,
    declared: &[String],          // raw `napi.targets` entries, Rust target triples
    root: &Path,
) -> Vec<Diagnostic>;
```

No `strict` parameter, for the same reason as `pre_mutation_checks` (§G.8.1): every
diagnostic here is emitted at `Warning` with `escalated_by: Some(StrictFlag::Strict)`, and
`escalate()` (§G.11) promotes it centrally at the command boundary.

```rust

/// `aarch64-apple-darwin` ⇄ `ManifestRole::Platform { platform: "darwin", arch: "arm64",
/// abi: None }`. Table-driven, covering napi-rs's supported target list; an unrecognised
/// triple produces a diagnostic naming it, never a panic and never a silent skip.
pub fn triple_to_role(triple: &str) -> Option<ManifestRole>;
pub fn role_to_triple(role: &ManifestRole) -> Option<String>;
```

> `[SPEC DECISION, not in 00-design.md: napi drift compares target **triples** via a localized
> mapping table, not derived package names.]` Deriving `@myorg/native-darwin-arm64` from
> `aarch64-apple-darwin` would couple callisto's steady state to `@napi-rs/cli`'s naming
> scheme — exactly the permanent coupling §5.3 says to avoid — and would fire false drift on
> any workspace that renamed a platform package. Triples are the stable identity; the mapping
> table is the one place callisto touches napi's conventions, deliberately localized in
> `napi.rs` and fixtured.

```
declared_triples ← { normalize(t) : t ∈ declared }
member_triples   ← { role_to_triple(m.role) : m ∈ group.members(PlatformManifest) }

for t ∈ declared_triples \ member_triples:
    Diagnostic { code: NapiTargetAddedNotInMembers, severity: Warning,
                 escalated_by: Some(StrictFlag::Strict),
                 message: "`napi.targets` declares `{t}`, which is not in fixed group
                           `{g}`'s members; accept it with `callisto init`" }

for t ∈ member_triples \ declared_triples:
    if the member's manifest exists on disk:
        Diagnostic { code: NapiTargetRemovedStillOnDisk, severity: Warning,
                     escalated_by: Some(StrictFlag::Strict), … }
    # A member whose manifest is missing entirely never reaches here: group *resolution*
    # (§G.5.5) already failed it as GraphError::MissingGroupMember — §7.5's "hard error
    # unconditionally, not a drift diagnostic."
```

§8 is explicit that this check firing every run for the duration of a pre-release cycle is
expected behaviour, with no pre-mode-specific suppression; nothing here consults `PreState`.

### G.9 Tags and change detection — `tags.rs`, `changed.rs`

#### G.9.1 `last_tag_for` — the git half (§9.1, §M.9.4)

`callisto-model` owns `TagTemplate::glob`, `TagTemplate::extract_version_str`, and
`select_last_tag` (steps 4–6). This crate owns steps 1–3, because they need `CommandRunner`:

```rust
/// §M.9.3 steps 1–3, then delegate. §13 invariant 25's "exactly one function" is satisfied
/// jointly by this and `select_last_tag`; there is no second glob-and-extract path anywhere.
pub fn last_tag_for<R: CommandRunner>(
    runner: &R,
    root: &Path,
    template: &TagTemplate,
    grammar: VersionGrammar,
) -> Result<LastTagSelection, GraphError> {
    let glob = template.glob();
    let out = runner.run("git", &["tag", "--list", &glob], root)?;
    // A non-zero exit here *is* a failure (unlike `CommandRunner`'s general contract, §M.10):
    // `git tag --list` with no matches exits 0 with empty output, so non-zero means the repo
    // is unreadable, not that there are no tags.
    if !out.success() { return Err(GraphError::Command(CommandError::Io { … })); }
    Ok(select_last_tag(template, grammar, out.stdout_lines())?)
}
```

Prereleases are **included**, never filtered (§M.9.3): a published prerelease is a real release
point with a real tag, and both §6.3's "changes since that package's last-release tag" and
§7.1's inference window want the most recent one.

#### G.9.2 `TagIndex`

```rust
/// One `git tag --list` per package, memoized for the whole run. Built once by
/// `Workspace::load`; consumed by aggregation (inference windows), §6.3's validation,
/// §G.8.2's released/fresh partition, `status`, and `plan-publish`.
pub struct TagIndex {
    last: BTreeMap<PackageId, Option<LastTag>>,
    templates: BTreeMap<PackageId, TagTemplate>,
    pre_cursor: BTreeMap<PackageId, Option<CommitSha>>,
    diagnostics: Vec<Diagnostic>,   // TagGlobNonVersionMatch, accumulated from every selection
}

impl TagIndex {
    pub fn build<R: CommandRunner, D: DependencyResolver>(
        runner: &R, root: &Path, graph: &D, cfg: &ResolvedConfig,
    ) -> Result<Self, GraphError>;

    pub fn last_tag(&self, id: &PackageId) -> Option<&LastTag>;
    /// The resolved template — `Package::tag_template` or §9.1's default. Every call site that
    /// needs a tag name goes through here and then `TagTemplate::render` (§M.9.2).
    pub fn template(&self, id: &PackageId) -> &TagTemplate;
    /// `refs/callisto/pre-cursor/<PackageId::display_name>` (§8), resolved via
    /// `callisto_conventional::resolve_pre_cursor` when the `inference` feature is on and
    /// always `None` otherwise. A missing ref means "absent," not an error — the ref namespace
    /// is bookkeeping about what inference already saw, not a release signal.
    pub fn pre_cursor(&self, id: &PackageId) -> Option<&CommitSha>;
}
```

A package's `TagTemplate` is resolved once, here, from `Package::tag_template` or §9.1's
default `{name}@{version}` with `{name}` substituted from the shortest unambiguous identity
form. The resolution is `callisto-model`'s (§M.9.1); this crate never assembles a tag from
`name + "@" + version` at any call site — §13 invariant 25 and release-please's `#2207` are the
reason, and a grep for `'@'` adjacent to a version in this crate is a review smell.

#### G.9.3 `changed_since_last_tag` (§6.3)

```rust
/// Does this package have file changes since its own last-release tag?
/// Input to §6.3's empty-changeset validation and to `StatusEntry::changed_since_last_tag`.
pub fn changed_since_last_tag<R: CommandRunner>(
    runner: &R, root: &Path, pkg: &Package, tags: &TagIndex,
) -> Result<bool, GraphError>;

/// The file scope of a package: the minimized set of directories containing its manifests.
/// "Minimized" = drop any path that has an ancestor also in the set, so a napi package whose
/// platform manifests live under its own root collapses to one path. Also the `pathspecs`
/// input to §C.5's inference window.
pub fn package_paths(pkg: &Package) -> Vec<PathBuf>;
```

```
match tags.last_tag(pkg.id):
    None      → Ok(true)     # never released: everything is new, and §7.1's pre-major
                             # bootstrap discussion depends on this being a routine state
    Some(t)   → git diff --quiet <t.name> -- <paths…>   # exit 1 = changed, 0 = unchanged
```

A package whose canonical manifest *is* the workspace root (a single-crate repo with
`[workspace]` and `[package]` in one file) has `package_paths == ["."]` and is therefore always
"changed." That is correct rather than degenerate — every commit in such a repo does touch the
package — and is called out so it is not later mistaken for a bug.

**§6.3's validation** then reads:

```
for cs in changesets:
    if no package named by `cs` has changed_since_last_tag:
        # §6.3: "escape hatch via `--allow-empty-changesets` OR
        # `[validation].allow-empty-changesets = true`" — the two are OR-ed, never a
        # precedence question (§G.11's `VersionOptions::allow_empty_changesets`).
        if cfg.validation.allow_empty_changesets || opts.allow_empty_changesets: skip
        else: Diagnostic { code: EmptyChangeset, severity: Warning,
                           escalated_by: Some(StrictFlag::Strict),
                           governed_by: Some(ConfigKey::VALIDATION_ALLOW_EMPTY_CHANGESETS),
                           path: Some(cs.path.clone()), … }
```

### G.10 The compute/apply split — `plan.rs`, `apply.rs`

#### G.10.1 `VersionPlan`

```rust
/// The pure product of `plan_version` (§0.1 rule 3, borrowed from release-please's
/// `buildPullRequests()`/`createPullRequests()`). Everything the apply step needs, and
/// nothing that requires re-deriving a release decision.
///
/// This is **not** a §12.5 contract type: `.callisto/plan.json` is `.gitignore`d and never
/// load-bearing (§13 inv. 4, decision doc rule 4). `VersionReport` (§M.12.3) is the contract,
/// and `to_report()` below produces it.
#[derive(Clone, Debug, Default)]
pub struct VersionPlan {
    /// Sorted by `PackageId`.
    pub bumps: Vec<PlannedBump>,
    /// Sorted by `RewriteKey`; already de-duplicated (§G.4.4, §G.7.3).
    pub rewrites: Vec<SpecRewrite>,
    /// §7.6 step 4 — one entry per `Platform` manifest, carrying the parent's new version.
    pub platform_writes: Vec<PlatformWrite>,
    /// §7.6 step 5 — napi main packages' `optionalDependencies`, at exact platform versions.
    pub optional_dep_updates: Vec<OptionalDepUpdate>,
    /// §7.6 step 7. Carries the `ChangelogInput` (§CL.3), not a pre-rendered string, so the
    /// rendering happens once, at apply time, through the one `render_section` (§CL.2).
    /// `plan-publish`'s `changelogSection` does **not** come from here — it is read back off
    /// the written `CHANGELOG.md` in a later process (§CL.7.1).
    pub changelog_writes: Vec<ChangelogWrite>,
    /// §7.6 step 8 — deleted only after every prior write succeeds.
    pub consumed_changesets: Vec<PathBuf>,
    /// §8 — `pre.json` rewrite, including synthesized `initialVersions` for new group members.
    /// `None` with `delete_pre_json = true` is the `mode: "exit"` compounding case (§G.6.8).
    pub pre_state_update: Option<PreState>,
    pub delete_pre_json: bool,
    /// §8 — `refs/callisto/pre-cursor/<id>` writes, for `Auto` packages bumped in pre-mode.
    pub pre_cursor_updates: Vec<(PackageId, CommitSha)>,
    /// §7.6 step 1's rerun-safety input: on-disk versions as observed at aggregation time.
    pub observed_versions: BTreeMap<PackageId, Version>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedBump {
    pub package: PackageId,
    pub from: Version,
    pub to: Version,
    pub severity: Severity,
    pub governed_by: Option<ConfigKey>,
    pub reason: Option<BumpReason>,
    /// Every version write this bump implies, as a **typed** target rather than a bare path.
    /// A Case D package has two entries. *Intended* (§CM.4.4): a Cargo member inheriting
    /// `version.workspace = true` has a `CargoWorkspacePackage` entry instead of a `Manifest`
    /// one. **Not implemented as of this writing** — `commands/version.rs`'s planner always
    /// emits `Manifest` entries regardless of inheritance; see §G.10.2 step 3's note for why
    /// this is safe rather than a live bug.
    pub writes: Vec<VersionWriteTarget>,
}

/// Where a *version* write lands. §7.6 step 3 dispatches on this directly (§G.10.2).
///
/// A bare `PathBuf` is ambiguous for the one case §G.2.2 explicitly supports: a Cargo root
/// that is **both** a workspace and a package. `root/Cargo.toml` is then a legal value for
/// both "write `[package].version` here" and "write `[workspace.package].version` here", and
/// those are two different keys in the same file, written through two different entry points
/// (`Manifest::write_version` vs. `WorkspaceCargoResolver::write_version`) — one of which the
/// other actively refuses (`ManifestError::WorkspaceInherited`, §CM.4.4). A root package that
/// itself declares `version.workspace = true` legitimately produces **both** entries, and the
/// enum is what keeps them from collapsing into one.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum VersionWriteTarget {
    /// Write this manifest's own version field — `[package].version` for `Cargo.toml`,
    /// `"version"` for `package.json` — via `Manifest::write_version` (§CM.1).
    Manifest(PathBuf),
    /// Write `[workspace.package].version` at this Cargo workspace root, via
    /// `WorkspaceCargoResolver::write_version` (§CM.4.4). Emitted once per bump per root,
    /// however many members inherit from it.
    CargoWorkspacePackage { root_manifest: PathBuf },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlatformWrite { pub manifest: PathBuf, pub version: Version }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OptionalDepUpdate { pub manifest: PathBuf, pub updates: Vec<(String, Version)> }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangelogWrite { pub changelog_path: PathBuf, pub input: ChangelogInput }

impl VersionPlan {
    /// §12.5's contract shape. `callisto-cli` serializes this; it never assembles one.
    pub fn to_report(&self, lockfiles: Option<Vec<LockfileRefreshResult>>) -> VersionReport;
}
```

#### G.10.2 `apply_version_plan` — §7.6's ordering, in this crate

```rust
/// §7.6 steps 1–9 and 11, in order. The orchestration loop lives in the core (decision doc's
/// semantic-release lesson: "the orchestration loop must live in the core"); the per-file
/// writes are delegated to `callisto-manifests` and `callisto-changelog` (decision doc change
/// 4), and step 10 (`.callisto/plan.json`) is `callisto-cli`'s, since whether a copy is
/// dropped on disk is an I/O concern and the file is never load-bearing.
pub fn apply_version_plan<R: CommandRunner>(
    root: &Path, plan: &VersionPlan, runner: &R, opts: &ApplyOptions,
) -> Result<ApplyOutcome, GraphError>;

#[derive(Clone, Debug, Default)]
pub struct ApplyOptions {
    pub refresh_lockfiles: bool,
    /// §8's `snapshot` mode: the writes are transient, uncommitted, and untagged, so steps 7
    /// (changelog prepend), 8 (changeset deletion, `pre.json`, pre-cursor refs) and 11 (`git
    /// add`) are suppressed. `false` for `version`, the only other caller. See the note below
    /// the step list for why this is not optional.
    pub transient: bool,
}

#[derive(Clone, Debug, Default)]
pub struct ApplyOutcome {
    /// `None` when `refresh_lockfiles` was false — the distinction §M.12.3's
    /// `Option<Vec<_>>` field exists to preserve.
    pub lockfile_refresh_results: Option<Vec<LockfileRefreshResult>>,
    /// Paths staged at step 11, for the caller's log.
    pub staged: Vec<PathBuf>,
}
```

```
 1. Rerun-safety (§13 inv. 12): re-read every package's on-disk version; compare to
    `plan.observed_versions`; any difference → GraphError::OnDiskVersionDrift, before any
    write. Compared against the *plan*'s observations, never against `.callisto/plan.json`,
    which is written later at step 10.
 2. Fixed-group alignment re-check + napi drift (§G.8) — verification, never a corrective
    write, with the single exception of the new-member force-set (§7.5), which is a write and
    happens at step 3/4.
 3. Version writes, from every `PlannedBump.writes` entry, dispatched on its
    `VersionWriteTarget` (§G.10.1):
      VersionWriteTarget::Manifest(p)
          → callisto_manifests::open(p, &ctx)?.write_version(&bump.to)
      VersionWriteTarget::CargoWorkspacePackage { root_manifest }
          → WorkspaceCargoResolver::load(root_manifest)?.write_version(&bump.to)
    *Intended* (§18 Q2, P5): `CargoWorkspacePackage` entries collected across all bumps,
    de-duplicated by root before writing — N members inheriting `version.workspace = true`
    all name the same root, one write serves all of them — with two bumps naming the same
    root at *different* `to` versions raising `GraphError::WorkspaceVersionConflict` (§G.12)
    before any write, since a workspace whose members share one inherited version cannot have
    them diverge and silently letting the last write win would pick a winner by iteration
    order. **Not implemented as of this writing**: `apply_version_plan` applies each
    `CargoWorkspacePackage` write independently, in bump order, with no de-duplication or
    conflict check — and moot in practice today, since `commands/version.rs`'s planner never
    actually constructs a `CargoWorkspacePackage` write target for any real plan (grep confirms
    zero production call sites); the type and this dispatch arm are exercised only by
    `apply.rs`'s own hand-constructed test fixtures. A workspace-inheriting Cargo member's
    version bump today instead always goes through the `VersionWriteTarget::Manifest(p)` arm
    above, which `CargoToml::write_version`'s own inherited-version branch
    (`callisto-manifests/src/cargo.rs`, tested by
    `write_version_pins_explicitly_on_workspace_inheriting_member`) handles safely on its own
    by writing an explicit pinned version directly into the member's `[package]` section,
    without touching the shared root at all — which is why the unimplemented dedup/conflict
    path above is unreachable rather than unsafe. (§CM.1's own prose describing
    `write_version` as refusing-and-redirecting for an inherited member is itself stale
    relative to this pin-explicitly behavior — tracked separately, not fixed by this note.)
 4. Platform manifests    → same, from `plan.platform_writes` (inherit parent version)
 5. optionalDependencies  → Manifest::update_optional_dependencies
 6. Dependency specs, from every `plan.rewrites` entry, dispatched on its `DepWriteTarget`
    (§G.7.3):
      DepWriteTarget::Manifest(p)
          → callisto_manifests::open(p, &ctx)?
                .update_dependency_spec(&r.key.name, r.key.kind.expect("Manifest ⇒ Some"), r.to)
      DepWriteTarget::CargoWorkspaceDependency { root_manifest }
          → WorkspaceCargoResolver::load(root_manifest)?.write_dependency(&r.key.name, r.to)
    Routing an inherited rewrite through `Manifest::update_dependency_spec` on the *member*
    is precisely the bug `ManifestError::WorkspaceInherited` exists to catch (§CM.4.4); with
    the target typed at plan time, that backstop should now be unreachable in normal operation
    — which is what makes a fixture that trips it (§G.15 item 6) meaningful.
 7. Changelogs            → callisto_changelog::render_section + prepend
                            (§CL.5/§CL.6 — `prepend` is the real file-writing one; there is no
                            pure string-splicing variant). **Skipped entirely when
                            `opts.transient`** (§8's snapshot mode, below).
 8. Delete consumed changeset files — only after 3–7 all succeeded.
    Write `pre.json` (or delete it, per `delete_pre_json`) and advance pre-cursor refs here
    too, in the same phase (§8). **The whole of step 8 is skipped when `opts.transient`.**
 9. Optional lockfile refresh (`--refresh-lockfiles`, off by default), via `runner`.
11. `git add` the modified paths. **Never commit** (§7.6 step 11). **Skipped when
    `opts.transient`** — a snapshot's writes are meant to be thrown away, and staging them
    would put a transient version into the next `git commit -a` a human runs.
```

**`ApplyOptions::transient` — snapshot mode (§8), and why it is a flag rather than a second
function.**

> `[SPEC DECISION, not in 00-design.md: `ApplyOptions` gains a `transient: bool` that
> suppresses steps 7, 8, and 11.]` §8 is explicit that `snapshot` "writes to manifests
> **uncommitted, untagged**" and that it computes "a transient, non-persistent version" — a
> reversible, throw-away workspace mutation. But §7.6's ordering, which `apply_version_plan`
> implements in full, *always* prepends changelog entries and *always* deletes the consumed
> changeset files. An unqualified `apply_version_plan` call from `snapshot` (§CLI.6.6) would
> therefore delete every pending changeset in the repository and write a `## 0.0.0-<tag>-<sha>`
> section into every `CHANGELOG.md` — destroying the release intent the *next* real `version`
> run depends on, with nothing left on disk to recover it from. That is data loss, not a
> policy difference. `plan_snapshot` (§G.11) additionally leaves `changelog_writes`,
> `consumed_changesets`, `pre_state_update`, and `pre_cursor_updates` empty, so the two
> mechanisms agree; the flag is the structural guarantee, the empty plan fields are the
> belt-and-braces. A separate `apply_snapshot_plan` function was rejected because it would be
> §7.6's ordering written twice, which is exactly the duplicated-orchestration shape §13
> invariant 25's reasoning warns about.

Every subprocess in steps 8, 9, and 11 goes through `CommandRunner`, whose contract already
forbids inheriting stdio (§M.10) — which is how §13 invariant 5 ("nothing but the intended JSON
reaches stdout") is satisfied structurally at the spawn site rather than by a caller's
checklist.

> `[SPEC DECISION, not in 00-design.md: `apply_version_plan` — §7.6's step ordering — lives in
> `callisto-graph`, not `callisto-cli` and not `callisto-manifests`.]` §7.6 specifies the
> ordering without assigning it a crate. It cannot live in `callisto-cli` (§13 inv. 27, and
> §G.1.7 removes the dependency that would make it possible); it cannot live in
> `callisto-manifests`, which is feature-flagged per ecosystem and knows nothing about groups,
> changesets, or `pre.json`. The decision doc's semantic-release analysis is explicit that the
> orchestration loop must be in the core, and this is that loop.

### G.11 Command compute functions — `commands/`

Each is a pure-as-possible function returning a §M.12 report type. `callisto-cli` parses argv,
constructs a `Workspace`, calls one of these, and renders. That is the whole of §15's "argv
parsing, rendering, and process I/O only."

```rust
pub struct Workspace<'a, R: CommandRunner, D: DependencyResolver = ManifestWalkResolver> {
    pub root: PathBuf,
    pub config: ResolvedConfig,
    pub graph: D,
    pub tags: TagIndex,
    pub runner: &'a R,
}

impl<'a, R: CommandRunner> Workspace<'a, R, ManifestWalkResolver> {
    pub fn load<L: ProjectLocator>(root: PathBuf, locator: &L, runner: &'a R)
        -> Result<Self, GraphError>;
}

impl<'a, R: CommandRunner, D: DependencyResolver> Workspace<'a, R, D> {
    /// Every package's **on-disk version of record**, read from its canonical manifests
    /// through `callisto-manifests` and reconciled per `Package::version_grammar` (§M.6.1) —
    /// a Case D package's two canonical manifests must agree, or
    /// `GraphError::FixedGroupDivergent`'s single-package analogue,
    /// `ModelError::MixedVersionGrammars`/`GraphError::SplitIdentity`, has already fired
    /// during construction.
    ///
    /// This exists because there is no other lawful way for a wrapper to learn a version.
    /// `Package` deliberately has no version field (§M.6.1's SPEC DECISION M6 — six fields,
    /// exactly §5.1's), and `callisto-cli` deliberately cannot depend on `callisto-manifests`
    /// at all (§G.1.7's SPEC DECISION G4, §13 inv. 27's structural half). `callisto-graph`
    /// **can** depend on `callisto-manifests`, so the read belongs here; without it,
    /// `callisto pre enter` (§8, §CLI.6.4) — whose entire job is "snapshot every package's
    /// current version into `initialVersions`" — has no reachable source for the thing it
    /// snapshots.
    ///
    /// Also the source of `CascadeInput::base` (§G.7.4) and §G.8.2's alignment input, so the
    /// on-disk read happens once per invocation rather than once per consumer (P2).
    pub fn base_versions(&self) -> Result<BTreeMap<PackageId, Version>, GraphError>;

    /// The `pre.json` `initialVersions` key for one package. See `initial_versions` for the
    /// rule and why it is not `PackageId::display_name`.
    pub fn pre_json_key(&self, id: &PackageId) -> Result<&str, GraphError>;

    /// `initialVersions` for `callisto pre enter` (§8, §CLI.6.4): every package's current
    /// on-disk version, keyed for the file and ordered deterministically by key, ready to
    /// hand to `PreState::entering` (§F.7).
    pub fn initial_versions(&self) -> Result<Vec<(String, Version)>, GraphError>;
}

// commands/
pub fn status  (ws: &Workspace<'_, impl CommandRunner, impl DependencyResolver>,
                opts: &StatusOptions)
    -> Result<StatusReport, GraphError>;
pub fn plan_version<I: SeverityInference>(ws: &…, inference: &I, opts: &VersionOptions)
    -> Result<VersionPlan, GraphError>;
pub fn plan_publish(ws: &…, opts: &PublishOptions) -> Result<PublishPlan, GraphError>;
pub fn plan_snapshot(ws: &…, tag: &str) -> Result<(VersionPlan, SnapshotReport), GraphError>;
pub fn validate(ws: &…, opts: &ValidateOptions) -> Result<ValidateReport, GraphError>;
pub fn create_tags(ws: &…, plan: &PublishPlan) -> Result<TagReport, GraphError>;
pub fn compose_pr_body<I: SeverityInference>(ws: &…, inference: &I, opts: &PrBodyOptions)
    -> Result<ComposePrBodyReport, GraphError>;
pub fn init(ws: &…, opts: &InitOptions) -> Result<InitReport, GraphError>;
pub fn matrix(ws: &…, opts: &MatrixOptions) -> Result<MatrixReport, GraphError>;

#[derive(Clone, Debug, Default)] pub struct StatusOptions {
    pub strict: bool, pub strict_graph: bool,
}
#[derive(Clone, Debug, Default)] pub struct VersionOptions  {
    pub strict: bool,
    /// §7.2's moon edge cross-check, escalated. Independent of `strict`; neither implies the
    /// other (§6.3's `--strict` composition paragraph).
    pub strict_graph: bool,
    /// §6.3's per-invocation escape hatch, OR-ed with
    /// `ResolvedConfig::validation.allow_empty_changesets`.
    pub allow_empty_changesets: bool,
}
#[derive(Clone, Debug, Default)] pub struct PublishOptions  { }
#[derive(Clone, Debug, Default)] pub struct ValidateOptions {
    pub staged: bool, pub since: Option<String>,
    pub strict: bool, pub strict_graph: bool,
}
#[derive(Clone, Debug, Default)] pub struct PrBodyOptions {
    pub existing_body: Option<String>, pub labels: Vec<String>, pub branch: Option<String>,
}
#[derive(Clone, Debug, Default)] pub struct InitOptions { pub yes: bool }
#[derive(Clone, Debug, Default)] pub struct MatrixOptions { pub package: Option<String> }
```

**Strictness escalation is one function, applied at the command boundary.** Graph construction
(§G.4.1) and the group checks (§G.8) emit their diagnostics at `Warning` with an
`escalated_by` flag naming which switch promotes them (§M.11.2); they do not — and for
`crosscheck_declared_edges`, cannot — know which flags the invocation carried, since
`ManifestWalkResolver::build` runs before any command function's options are consulted. Each
command function therefore applies:

```rust
/// Promotes every diagnostic whose `escalated_by` names an enabled flag to
/// `DiagnosticSeverity::Error`, in place. Applied exactly once per command, to the report's
/// assembled `diagnostics` array, so a diagnostic's severity in `--format json` and the
/// command's exit code (§CLI.7) can never disagree.
pub fn escalate(diagnostics: &mut [Diagnostic], strict: bool, strict_graph: bool);
```

> `[SPEC DECISION, not in 00-design.md: `commands::init` exists here, with the signature
> above.]` §17/§18 Q5.5 describe `init`'s behaviour in detail (discovery, derivation, diff
> generation, reviewable writes, `--format json`, `--yes`) without naming a crate. Per P6,
> discovery/derivation/diff-generation is release-adjacent coordination logic and belongs in
> the core, not in a wrapper; and both wrappers need it (§CLI.6.7's `callisto init` and
> §MO.2.4's `initialize_extension` are two callers of one function, §MO.8). §11 records this,
> since one draft assumed it and another's module listing omitted it.

> `[SPEC DECISION, not in 00-design.md: `pre.json`'s `initialVersions` is keyed by
> **ecosystem-native package name**, and that mapping is computed here, in `callisto-graph`,
> not in a wrapper.]` §6.4 makes `pre.json` byte-shape-compatible with `@changesets/cli` and
> P1 makes that the adoption gate, so the keys have to be what `@changesets/cli` itself writes:
> bare, native package names (`"@myorg/sdk"`, `"engine"`) — never
> `PackageId::display_name()`'s prefixed `"cargo/foo"` form, which no `@changesets/cli` would
> ever produce or read, and which would silently break the one-commit-rollback promise for
> exactly the workspaces (cross-ecosystem name collisions, §5.4) that most need it. An earlier
> draft had `callisto-cli` build the map keyed on `display_name()`; the key rule is a
> workspace-aware identity question, so it belongs with the identity index (§G.4.2), not in a
> wrapper.
>
> **The rule**, for a package `p`:
> 1. If `p` has a canonical `package.json`, its key is `native_name(p, Ecosystem::Npm)`. npm
>    is preferred for a Case D package because `@changesets/cli` only ever knew npm names —
>    it is a JS-only tool (§0) — so the npm name is the one a migrating repo's existing
>    `pre.json` already contains, and matching it is what byte-compat means here.
> 2. Otherwise its key is `native_name(p, eco)` for its single canonical ecosystem.
> 3. Two packages resolving to the same key is `GraphError::DuplicatePackage` — unreachable
>    in practice, since native names are unique per ecosystem by construction (§G.4.3) and
>    rule 1 makes the choice deterministic, but checked rather than assumed, because a
>    silently-collapsed `initialVersions` entry would make one package bump from the other's
>    baseline for a whole pre-release cycle.
>
> Reading a key back (`pre` mode's per-package `base` lookup, §G.6.8) uses the same function,
> so the write and read sides cannot drift — §13 invariant 25's discipline applied to a third
> identity-shaped mapping.

Five of these have behaviour worth pinning here rather than leaving to the milestone:

**`plan_publish` (§9.2, §13 inv. 7/8/20).** The release set is the set of packages whose
on-disk version differs from their last release tag's version (P2: stateless, compare disk to
tags — never a state file). Every one of §9.2's four arrays is derived from it as follows.

`rustCrates[]` — the release set filtered to `PublishTarget::CratesIo`, ordered by
`graph.toposort(&release_set)`; the array order **is** the topological order, scoped to the
intra-release set (§13 inv. 7). `name` is `IdentityIndex::native_name(id, Ecosystem::Cargo)`.

`npmMainPackages[]` — the release set filtered to an npm publish target, `name` from
`native_name(id, Ecosystem::Npm)`.

`npmPlatformPackages[]` — **not derived from the release set's `Package`s**, because a napi
platform package is not a `Package` at all (§M.6.1's SPEC DECISION M7); it is a
`ManifestRole::Platform` manifest belonging to its main package, and no per-`Package`
iteration will ever visit one. It is derived instead as:

```
for pkg in release_set where pkg publishes to npm:
    for (name, manifest_path) in index.platforms_of(&pkg.id):        # §G.4.2
        npm_platform_packages.push(NpmPublish {
            name,                              # the platform manifest's OWN registered name,
                                               # from IdentityIndex::platform — never derived
                                               # from a target triple (§G.8.4's decision) and
                                               # never from the parent's name plus a suffix
            version:    versions[&pkg.id],     # inherited from the parent, unconditionally
                                               # (§7.6 step 4) — a platform manifest has no
                                               # independent version line to read
            publish_to: registry_key_of(&pkg),  # the PARENT's npm target; a platform package
            registry:   registry_url_of(&pkg),  # publishes wherever its main package does
        })
sort npm_platform_packages by name              # determinism, §G.1.8
```

`NpmMainPublish::depends_on_platforms` is the *same* list, projected to names, for that main
package only — computed from one `platforms_of` call so the two can never disagree, which is
what makes §CF.5's "every name in `dependsOnPlatforms` appears in `npmPlatformPackages[]`"
property hold by construction rather than by fixture luck. It is empty for a non-napi package.

`releases[]` — built from `Package::is_release_point` (§M.6.1), which is what makes §13
invariant 20 ("platform manifests never receive an independent git tag") structural rather
than a filter someone must remember to apply; platform manifests are excluded automatically,
since they are not `Package`s. `changelog_section` is read back off disk per §CL.7.1.

`npmPlatformPackages` precedes `npmMainPackages` structurally, and no config reorders it (§13
inv. 8).

> `[SPEC DECISION, not in 00-design.md: `npmPlatformPackages[]`'s derivation, and
> `depends_on_platforms`'s, are specified here as a manifest-level walk rather than a
> `Package`-level one.]` §9.2 shows the array's *shape* and §13 inv. 8 fixes its *order*, but
> no section says where the entries come from — and under M7 the obvious reading ("one entry
> per platform package in the release set") names a set that is always empty, because platform
> packages are not `Package`s and therefore never enter a release set. Sourcing name from
> `IdentityIndex::platform`, version from the parent, and `publishTo` from the parent is the
> only derivation consistent with M7, with §7.6 step 4's unconditional inherit-parent-version
> write, and with §7.5's "platform packages are dependents-in-lockstep, not independently
> released artefacts."

**`create_tags` (§9.1, §13 inv. 16/24).** Creates **local tags only**. Never pushes. An existing
tag at the same sha → `CreatedTag { already_existed: true }` (P3's idempotence made observable);
an existing tag at a *different* sha → an error diagnostic, never a silent `-f`.

**`compose_pr_body` (§13 inv. 23, §12.2).** Reads the changeset files directly and never
`.callisto/plan.json`. It must run before `version`, which deletes those files; it takes no
`VersionPlan` argument, so there is no signature by which it could depend on `version` having
run.

It **does** compute prospective versions, by calling `plan_version` itself:

```rust
let plan = plan_version(ws, inference, &VersionOptions::default())?;   // pure — computes only
for bump in &plan.bumps {
    // <details><summary>{native_name}@{bump.to}</summary> … </details>
    // body from render_section(&ChangelogInput { to: Some(bump.to.clone()), .. })
}
```

> `[SPEC DECISION, not in 00-design.md: `compose-pr-body`'s preview renders **real prospective
> versions**, obtained by calling `plan_version`, rather than a version-less `## Unreleased`
> heading.]` §12.2 requires "one `<details><summary>package@version</summary>` collapsible
> section per package," and the whole stated purpose of that shape — release-please's pattern,
> adopted because "collapsed-by-default reads better for callisto's typically-larger polyglot
> monorepos" — is that a reviewer can see *what version they are approving* before merging the
> release PR. A summary line with no version in it does not do that. An earlier draft passed
> `to: None` on the grounds that the target version "is not yet final because `version` has not
> run"; that reasoning does not survive §0.1 rule 3's compute/apply split, which exists
> precisely so that a plan can be computed without being applied. `plan_version` is a pure
> function of on-disk state (§G.10.1: "the pure product of `plan_version`"), it writes nothing,
> and calling it here neither violates §13 invariant 23 (nothing is applied, no changeset is
> deleted) nor duplicates logic (P6 — the same function `version` calls). The version a
> reviewer sees is then the version they get, computed by the same code path, which is a
> stronger guarantee than the old wording offered.
>
> `ChangelogInput::to` stays `Option<Version>` (§CL.3): it is `None` only when there is
> genuinely no computable target — a package that appears in the PR body for its changeset
> text but that `plan_version` produced no bump for (an all-`none`-severity changeset, §6.1) —
> and `## Unreleased` remains the rendering for that residual case (§CL.5), not the default.

**`plan_snapshot` (§8).** Hard-errors when `pre.json` exists with `mode: "pre"` — the two
compute a version by different, incompatible rules from the same on-disk state, and the
decision doc's group-priority analysis names this mutual exclusion as one of the three reasons
callisto needs no arbitration mechanism.

> `[SPEC DECISION, not in 00-design.md: the snapshot version is exactly
> `0.0.0-{tag}-{sha7}`, identical for every package in the workspace.]` §8 gives only the
> example `0.0.0-snapshot-<sha>` with no composition rule, and "how do `--tag` and the sha
> combine" has at least three plausible answers (`-{tag}.{sha}`, `-snapshot-{tag}-{sha}`,
> `-{tag}+{sha}`) that produce different, non-interchangeable registry versions. Pinned:
>
> - **Base is literally `0.0.0`**, not the package's own version. That is what makes a
>   snapshot unmistakable and unpublishable-over-a-real-release by construction: every real
>   version sorts above it, so a snapshot accidentally left on a registry can never win
>   resolution against a genuine release. §8's own example already has this shape.
> - **`{tag}` is `--tag`'s value**, validated to `[0-9A-Za-z-]+` — the same rule as `pre
>   enter`'s tag (§CLI.6.4), rejected before any write otherwise. §8's `0.0.0-snapshot-<sha>`
>   is what `--tag snapshot` produces; `--tag pr-1234` produces `0.0.0-pr-1234-a1b2c3d`.
> - **`{sha7}` is `CommitSha::short()` of HEAD** — 7 characters, git's own default
>   abbreviation length (§M.2), resolved once via `git rev-parse HEAD` through
>   `CommandRunner`.
> - **The two are joined with `-`, inside a single SemVer prerelease identifier**, not with
>   `.`. A dot would make `{sha7}` its own identifier, and a sha that happens to be all
>   digits with a leading zero (`0123456`) is then an *invalid* numeric identifier under
>   SemVer — a once-in-a-few-hundred-runs hard failure that would look like a callisto bug.
>   One hyphen-joined alphanumeric identifier is always legal.
> - **Every package gets the identical string.** A snapshot is a coordinate for one workspace
>   state, not a per-package version line; per-package snapshot versions would make the
>   cross-registry `optionalDependencies` pinning (§7.6 step 5) resolve against versions that
>   do not exist yet at the moment the plan is composed.
>
> `SnapshotReport.version` (§M.12.5) carries this one value; `packages[]` carries the affected
> names and ecosystems, no per-package version, for the same reason.

**`matrix` (§19, §M.12.7, §CLI.6.13).** Read-only and on-demand: unlike the four functions above, it
does not participate in the release cycle, consumes no changesets, and touches no `TagIndex`.
For each package in scope (all registered packages, or exactly one under `opts.package`, which
is `Err(GraphError::UnknownPackage)` if the name doesn't match anything — checked before any
manifest is read, so an unknown `--package` never reaches a parse), it reads at most one
`package.json` and one `pyproject.toml`, shares each parsed value between the platform-target
and runtime-version derivations for that package (§G.1's P2 — one read, not one per
consumer), and produces two independent map entries: `platformTargets[name]` from
`napi.targets`/`[tool.maturin].targets`, and `runtimeVersions[name]` from
`engines.node`/`requires-python`. A package can contribute to either map, both, or neither; the
two are computed and reported independently.

A platform-target entry's `targets[]` comes from a per-triple join of two independently-pinned
tables: `napi.rs`'s `triple_to_role` (platform/arch/abi — shared with §G.4.5's unrelated napi
platform auto-derivation, which this module does not otherwise touch) and a table of exactly 18
`(hostRunner, useCross)` pairs new to this module (§G.11's justification: `ManifestRole::Platform`
carries no CI-scheduling information, so it cannot be derived from the shared table alone).
`artifactName` is always `"native-" + triple`. A triple absent from *either* table is excluded
from `targets[]` and reported as an `UnrecognisedPlatformTriple` diagnostic naming the triple and
the package — never a hard error, since one unrecognised triple in a declaration must not hide
the ones that were recognised. A triple repeated within one package's own declaration is
likewise not an error: the duplicate is dropped (first occurrence wins) and reported as
`DuplicatePlatformTriple`, since a hand-maintained `napi.targets` array is exactly the kind of
list a copy-paste duplicates without anyone noticing, and two identical `PlatformTarget` entries
would mean two identical CI jobs racing on the same artifact upload.

A package declaring platform targets via **both** `napi.targets` and `[tool.maturin].targets` —
even as an explicitly empty array on one side — is `Err(GraphError::ConflictingPlatformTargetSources)`
(AC-017): there is no principled way to prefer one source over the other, so neither is used.
A runtime-version entry orders `engines.node` before `requires-python` when a package declares
both (AC-005b) — arbitrary as a tiebreak, but fixed so `--format json` output is deterministic
run to run.

> `[SPEC DECISION, not in 00-design.md: neither `UnrecognisedPlatformTriple` nor
> `DuplicatePlatformTriple` carries an `escalated_by`, and `matrix`'s command handler never
> calls §G.11's `escalate`.]` Every other command function's diagnostics are subject to
> `--strict`/`--strict-graph` promotion (§G.11's opening paragraph); `matrix` has neither flag
> on `MatrixOptions` at all. `matrix` is a read-only discovery report consumed by CI tooling
> that expects a matrix even when some packages have manifest quirks — a workspace with one
> unrecognised triple should still emit a usable matrix for every other package, not fail the
> whole CI run on the strength of an unregistered target platform. Malformed manifest *syntax*
> (§AC-010/010b/010c) is still a hard `Err`, since there is no report to degrade to once a
> `package.json`/`pyproject.toml` doesn't parse at all — the diagnostic path exists specifically
> for the "each half parsed fine, this platform triple is merely unrecognised" case.

### G.12 Errors — `error.rs`

`LocateError` and `GraphError` are declared here, verbatim as pinned in §M.13.3. That text is
authoritative for every variant listed there; this section adds one enum and records the
`GraphError` variants that come with it.

> `[SPEC DECISION, not in 00-design.md: `GraphError` gains a `Config(#[from] ConfigError)`
> variant.]` §M.13.3 pins `GraphError`'s variants for cross-crate vocabulary agreement, and
> none of them can carry §14's config failures (a `[[package-set]]` matching nothing, an
> unknown registry key, a malformed `callisto.toml`). The enum is `#[non_exhaustive]`, which is
> the mechanism for exactly this; adding a transparent wrapper rather than N variants keeps
> §M.13.3's list intact and keeps config errors namespaced. The same reasoning covers the
> `Changelog` and `Conventional` transparent wrappers §M.13.3 also lists.

```rust
#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConfigError {
    #[error("failed to read `{path}`: {message}")]
    Read { path: PathBuf, message: String },

    #[error("`{path}` is not valid TOML: {message}")]
    ParseToml { path: PathBuf, message: String },

    /// §14. A set that matches nothing is almost always a typo'd glob, and silently ignoring
    /// it means a whole directory of packages never gets versioned.
    #[error("[[package-set]] `{pattern}` matched no packages")]
    PackageSetMatchedNothing { pattern: String },

    #[error("[[package]] `{pattern}` matched no package")]
    PackageMatchedNothing { pattern: String },

    /// §14: "two sets claiming the same package is a hard config error."
    #[error("package `{package}` is claimed by more than one [[package-set]]: {}",
            .patterns.join(", "))]
    OverlappingPackageSets { package: String, patterns: Vec<String> },

    /// §G.5.5 pass 1 — the syntactic half of §14's group validation, which runs before any
    /// discovery so a config that is wrong on its face fails on its own terms.
    #[error("group `{group}` and group `{other}` both list `{member}`")]
    ConflictingGroupNames { group: GroupName, other: GroupName, member: String },

    #[error("group `{group}` has no members")]
    EmptyGroup { group: GroupName },

    #[error("duplicate group name `{group}`")]
    DuplicateGroupName { group: GroupName },

    #[error("`publish-to` names registry key `{key}`, which no [registries.*] block defines")]
    UnknownRegistry { key: String },

    /// A typo'd override that silently does nothing is the failure P5 exists to make
    /// structural — so unknown keys under `extensions.callisto` are rejected, not ignored.
    #[error("`{path}` sets unknown callisto key `{key}`")]
    UnknownKey { path: PathBuf, key: String },

    /// §14's `[cascade].bump-severity` accepts `patch | minor` only (§G.5.4).
    #[error("`cascade.bump-severity` is `{found}`; expected `patch` or `minor`")]
    InvalidBumpSeverity { found: String },

    /// §G.5.3's three-valued key.
    #[error("`pre-major-inference` is `{found}`; expected `off`, `conservative`, or \
             `conservative-feat`")]
    InvalidPreMajorInference { found: String },

    /// `[changesets].dir` (`config/raw.rs`'s `RawChangesetsConfig.dir` doc comment) rejected
    /// for being absolute or containing `..` components — same portability/escape rule as
    /// `[[package]]`/`[[package-set]]` `changelog` below, applied to the workspace-root-
    /// relative changesets directory instead of a package-root-relative changelog path.
    #[error("changesets.dir `{dir}` is an absolute path or contains `..` path components and \
             would escape the workspace root")]
    InvalidChangesetsDir { dir: String },

    /// `[[package]]`/`[[package-set]]` `changelog` (`config/raw.rs`'s `RawPackageConfig`/
    /// `RawPackageSetConfig.changelog` doc comments) rejected for being absolute or containing
    /// `..` components — a value that could otherwise resolve outside the package root once
    /// joined with it.
    #[error("`changelog = \"{value}\"` on `{pattern}` is an absolute path or contains `..` \
             path components and would escape the workspace root")]
    InvalidChangelogPath { pattern: String, value: String },

    #[error(transparent)]
    Tag(#[from] TagTemplateError),

    /// Transparent from `callisto-model`'s `VersionParseError`. As of this writing, no
    /// production code path in this crate's config module actually parses a `Version` from
    /// config text (grep confirms no `Version::parse`/`VersionParseError` reference anywhere
    /// under `config/`), so this variant is currently unconstructed — the `#[from]` conversion
    /// exists but nothing triggers it yet.
    #[error(transparent)]
    VersionParse(#[from] callisto_model::VersionParseError),
}
```

`TagTemplateError` surfacing through `ConfigError` is deliberate: §9.1 and §M.9.1 both require
tag templates to be validated **at config-load time** (the `{version}`-exactly-once rule, the
glob-metacharacter rule, and §M.9.1's `NoLiteralAnchor` rule), because `last_tag_for` derives a
glob from the template and a template with no literal anchor would glob every tag in the
repository.

### G.13 Diagnostic attribution map

Every `DiagnosticCode` this crate emits, with the flag that escalates it and the key that
governs it — the concrete form of §13 invariant 28 applied to diagnostics.

| Code | Emitted by | `escalated_by` | `governed_by` |
|---|---|---|---|
| `EmptyChangeset` | §G.9.3 | `Strict` | `VALIDATION_ALLOW_EMPTY_CHANGESETS` |
| `GraphEdgeDisagreement` | §G.4.6 | `StrictGraph` | — |
| `RangeNotRoundTrippable` | §G.7.3, §G.7.7 | — | `CASCADE_PRESERVE_NPM_RANGES` |
| `CatalogSpecNotRewritten` | §G.7.3 | — | `CASCADE_PRESERVE_NPM_RANGES` |
| `NapiTargetAddedNotInMembers` | §G.8.4 | `Strict` | — |
| `NapiTargetRemovedStillOnDisk` | §G.8.4 | `Strict` | — |
| `NapiCoordinationNotYetSupported` | §G.4.5 (v0.1/v0.2 only) | — | — |
| `TagGlobNonVersionMatch` | §G.9.2 (via `select_last_tag`) | — | `TAG_TEMPLATE` |
| `PreMajorInferenceInert` | §G.6.3 (v0.2+) | — | `PRE_MAJOR_INFERENCE` |
| `ChangelogSectionNotFound` | §G.11 `plan_publish` (§CL.7.1, v0.2) | — | — |
| `ChangesetsConfigKeyDropped` | `init` (§18 Q4, v0.4) | — | — |
| `BareRuleMatchesMultipleEcosystems` | §G.4 walk | — | — |
| `UnrecognisedPlatformTriple` | §G.11 `matrix` | — | — |
| `DuplicatePlatformTriple` | §G.11 `matrix` | — | — |

`RangeNotRoundTrippable` and `CatalogSpecNotRewritten` carry no `escalated_by`: §13 invariant 15
makes leave-alone-and-warn the *correct* outcome, not a tolerated one, so there is no flag that
should turn it into a failure.

### G.14 Milestone slicing

| Module | v0.1 | v0.2 | v0.3 |
|---|:--:|:--:|:--:|
| `config` (less groups) | ✓ | | |
| `config::groups` | syntactic pass only | | full resolution |
| `locate`, `identity`, `walk`, `resolver`, `toposort` | ✓ | | |
| `crosscheck` | — (no locator supplies edges until v0.4) | | |
| `aggregate` (changesets), `infer::NoInference` | ✓ | | |
| `infer::CommitInference` + the `inference` feature, `apply_pre_major` | | ✓ | |
| `cascade` — all six rows, `mode`, `bump-severity`, peer-escalation | ✓ | | |
| `groups` — alignment, exemption, target version | | | ✓ |
| `napi` — drift; `NapiCoordinationNotYetSupported` diagnostic | detection only | | full |
| `tags::last_tag_for`, `TagIndex::last_tag` | ✓ | | |
| `tags::pre_cursor` | | | ✓ |
| `changed::changed_since_last_tag`, `package_paths` | ✓ | | |
| §6.3's empty-changeset *validation* (the check, the diagnostic, `allow-empty-changesets`) | | ✓ | |
| `commands::status`, `plan_version`, `init`, `apply` | ✓ | | |
| `commands::plan_publish`, `validate`, `create_tags`, `compose_pr_body`, `plan_snapshot` | | ✓ | |
| `commands::matrix` (§G.11, §CLI.6.13) | | | ✓ |

§17 v0.1's "all six edge-kind/coverage rows ship here since neither peer-escalation nor
dev-none is napi-specific" is why `cascade.rs` is complete at v0.1 while `groups.rs` is not.

**Why `changed` splits across the two milestones.** `StatusReport.packages[].
changed_since_last_tag` is a **mandatory** field of a v0.1 report (§M.12.4), so its computation
has to ship at v0.1 — an earlier draft of this table put the whole `changed` module at v0.2,
which would have left a v0.1 `status` unable to populate a field its own struct declares. That
is not a conflict with §17, only a finer reading of it: §17 v0.1 already commits the stateless
`last_tag_for` primitive "built and used by `status`", and `changed_since_last_tag` is one
`git diff --quiet <tag> -- <paths>` on top of it — the cheap half. What §17 v0.2 and §13
invariant 19 actually schedule is §6.3's **validation** — the cross-check that a changeset's
named packages actually changed, its `EmptyChangeset` diagnostic, and the
`allow-empty-changesets` escape hatch — which is the half that needs a policy decision and a
diagnostic surface, and which invariant 19 is careful to describe as "the finished tool's
steady-state behaviour, not v0.1's." v0.1 therefore *reports* the fact and does not *enforce*
it, which is exactly what a read-only `status` should do.

### G.15 Fixture obligations

`callisto-fixtures` must carry, for this crate specifically:

1. **Cascade table, exhaustively.** All `DepKind` × `Coverage` × `{Patch, Minor, Major}` source
   × `CascadeMode` × `peer_escalation` combinations against `cascade_action`, as a table test
   with the six §7.4 rows named. Includes the two §G.7.1 spec decisions as explicit cases:
   `mode = always` + covering peer + major source → `bump_severity`, not `Major`; `Unknown`
   coverage → never `rewrite`, `Covers`-like under `out-of-range`, `DoesNotCover`-like under
   `always`. **And the non-inertness case specifically**: `mode = always` + `Runtime` edge +
   `Coverage::Covers` → `severity == bump_severity` (not `Severity::None`), `rewrite == false`,
   `governed_by == Some(CASCADE_MODE)`. That one row is the whole content of the `[cascade].mode`
   config key; a `mode` that produced `Severity::None` there would be a key that does nothing,
   and this is the fixture that says so.
1b. **`coverage` per `DepSpec` variant** (§G.7.2). One row per variant, and specifically the
   `CargoBare` caret table: `("1.2.3", 1.2.9) → Covers`, `("1.2.3", 1.3.0) → Covers`,
   `("1.2.3", 2.0.0) → DoesNotCover`, `("0.2.3", 0.2.9) → Covers`, `("0.2.3", 0.3.0) →
   DoesNotCover`, `("0.0.3", 0.0.4) → DoesNotCover`, plus the same versions as
   `DepSpec::Exact` asserting `Covers` **only** on exact equality. This pair is the fixture
   that would catch `CargoBare` being treated as an exact pin — a bug whose blast radius is
   every ordinary `foo = "1.2.3"` dependency in every Cargo workspace, cascading a bump and a
   spec rewrite onto every dependent on every in-range bump.
2. **Fixpoint convergence and attribution.** A chain `a → b → c → d` where each hop is
   out-of-range, asserting one bump per package and `BumpReason::Cascade { via }` naming the
   correct predecessor; a diamond, asserting the attribution is the lowest-`PackageId`
   predecessor (determinism, §G.7.5); a graph engineered to require exactly `3n` raises,
   asserting `iterations ≤ convergence_bound(n)`.
3. **Peer escalation into a fixed group.** The §G.7.5 case: two fixed members, one
   peer-escalated to `Major`, asserting both end at `Major` and that no `FixedGroupDivergent` is
   raised. This is the fixture that would fail if `raise` were not group-aware.
4. **Linked-group joint vs. cascade.** Two linked members at different base versions: (a) both
   named → shared max version (§G.6.7); (b) one named, the other reached only by cascade →
   independent version lines, no union; (c) one named `none`, one named `minor` → joint, both
   `minor` (§G.6.6).
5. **Rewrite round-trip corpus.** `^1.2`, `~1.2.3`, `>=1.0 <2`, `1.2.3`, bare Cargo `1.2.3`,
   `workspace:*`, `catalog:default`, `^1 || ^2`, against a bump — asserting precision
   preservation, `LeftAlone` for the last, and that every `Rewritten` output re-parses and
   re-covers (§G.7.7's verification).
6. **Cargo workspace inheritance.** Three members inheriting one root dependency — **one of
   them under `[dev-dependencies]`** — asserting: exactly **one** `SpecRewrite`, whose
   `RewriteKey.target` is `DepWriteTarget::CargoWorkspaceDependency { root_manifest }` and
   whose `kind` is `None` (§G.7.3); that the dev-inheriting member's `DepEdge.kind` is
   nonetheless `Dev` and takes §7.4's Dev row (`Severity::None`, no version bump) while the
   other two bump — the kind-preservation contract §CM.4.4's `inherited()` exists to make
   structural; that the write lands in the root file, format-preserved; and that a direct
   write attempt against a member still produces `ManifestError::WorkspaceInherited` as the
   backstop.
6b. **Typed version write targets.** *Required, not yet present.* A Cargo root that is both a
   workspace and a package (§G.2.2), itself declaring `version.workspace = true`, asserting its
   `PlannedBump.writes` contains **both** `VersionWriteTarget::Manifest(root)` and
   `::CargoWorkspacePackage { root_manifest: root }` and that apply writes two different keys
   in one file. Plus the conflict case: two members inheriting one root, planned to different
   versions → `GraphError::WorkspaceVersionConflict` before any write. As of this writing,
   neither `data/manifests/cargo-workspace-inherit/` nor any matching test exists (confirmed by
   direct search of `callisto-graph/tests/`) — consistent with `commands/version.rs`'s planner
   never constructing a `CargoWorkspacePackage` write target at all (§G.10.2 step 3's note).
   This fixture would be the thing that actually exercises the currently-dead dedup/conflict
   path if that path is ever wired into the planner.
7. **Group validation.** One fixture per `ConflictingGroupNames` / `ConflictingGroupMembership`
   shape, including the identity-level case (`cargo/foo` in a fixed group, `npm/foo` in a linked
   one) that pass 1 cannot catch and pass 2 must.
8. **`toposort`.** A dev-dependency cycle that must **not** fail; a runtime cycle that must; a
   napi shape asserting platforms precede their main package; a tie-break case asserting
   `PackageId` order.
9. **`declared_edges` cross-check.** Both disagreement directions, plus a `Root`-scope edge and
   an edge to a manifest-less project, both of which must produce no diagnostic.
10. **Empty-changeset validation.** A changeset naming an unchanged package (warn), the same
    under `--strict` (error), the same with `allow-empty-changesets = true` in config
    (silent), and the same with `--allow-empty-changesets` on the command line and the config
    key `false` (silent) — the OR §6.3 names two spellings for.
10b. **New-member force-set.** A fixed group with one released member at `1.4.0` and one
    member at the scaffolding placeholder `0.0.0` with no tag: assert the new member's target
    is the group's target (`1.5.0` for a minor), **not** `0.0.1`, that no
    `FixedGroupDivergent` is raised, and that its `BumpReason` is `NewGroupMember`. This is
    the fixture that fails if `bump_target` stops delegating to `fixed_group_target` (§G.7.4,
    §G.8.3) and goes back to bumping each member from its own base.
10c. **Snapshot is non-destructive.** `plan_snapshot` + `apply_version_plan` with
    `transient: true` against a workspace with two pending changesets and existing
    `CHANGELOG.md` files: assert manifests were rewritten to `0.0.0-<tag>-<sha7>`, and that
    **both changeset files still exist**, no `CHANGELOG.md` changed, no `pre.json` was
    written, and nothing was staged. The same plan applied with `transient: false` deletes
    them — asserted too, so the flag is shown to be what does the work.
10d. **Pre-major boundary corpus** (§7.1, §G.5.3/§G.6.3 — moved here from
    `callisto-conventional`, which no longer applies this policy). The exact cases §7.1 calls
    out as release-please's leaked bug surface: `0.0.5` with a prior tag (inert), `0.3.0` with
    no prior tag (inert), `0.3.0` with a prior tag and `breaking_to_minor` on (remaps), `1.2.0`
    with `breaking_to_minor` on (inert — not pre-major at all), and both bools independently
    toggled against the same raw severity to confirm they gate independently. Run directly
    against `apply_pre_major` as a pure function — no `CommandRunner`, no real inference —
    since that is now this function's only production call site's shape (`CommitInference`,
    §G.6.4) plus this test itself.
11. **`wasm32-wasip1` fixture run.** The whole suite under `wasmtime` with only the workspace
    root preopened (§0.1 rule 2, §13 inv. 26), from v0.1. This crate's git-touching tests run
    against a `CommandRunner` fixture that replays canned `git` output, so the suite needs no
    process execution under WASI at all — which is the point: the seam is what makes it
    possible, and the fixture is what proves the seam is real.
12. **`xtask dep-audit` self-test.** A fixture manifest that *does* reach a forbidden crate,
    asserting the audit fails. An enforcement mechanism nobody has watched fail is not an
    enforcement mechanism.

### G.16 Index of `[SPEC DECISION]` flags

| # | Section | Decision |
|---|---|---|
| 1 | §G.1.3 | The entry points this crate requires from `callisto-manifests`/`callisto-format`/`callisto-changelog`/`callisto-conventional` are enumerated. |
| 2 | §G.1.4 | Config resolution lives in `callisto-graph::config`, not a tenth crate. |
| 3 | §G.1.7 | Zero-moon is enforced by an `xtask dep-audit` CI job over `cargo metadata`'s resolve graph (not feature flags, not a lint). |
| 4 | §G.1.7 | §13 inv. 27 is enforced structurally too: `callisto-cli` must not depend on `callisto-manifests`, which forces `ManifestWalkResolver::build` to own `OpenContext` construction. |
| 5 | §G.2.3 | `pnpm-workspace.yaml` is a fourth workspace-root marker; §M.13.3's message text reflects it. |
| 6 | §G.3.2 | `toposort` uses `Runtime \| Build \| Optional` edges and excludes `Dev \| Peer`. |
| 7 | §G.4.2 | `IdentityResolver` is public so `MoonProjectLocator` reuses the one identity path (struct form, not a free function taking a `callisto-manifests` type). |
| 8 | §G.4.4 | `DepEdge::from_manifest` for a Cargo-inherited dependency is the workspace root; rewrites de-duplicate on `(manifest, name, kind)`. |
| 9 | §G.4.6 | `DeclaredEdgeKind::Root` edges and edges with an unresolvable endpoint are excluded from the cross-check, on the consuming side. |
| 10 | §G.5.2 | `moon.yml`'s `extensions.callisto` block has the highest config precedence and is read by the core as plain YAML. |
| 11 | §G.5.3 | `pre-major-inference` is a three-valued string key (`off`/`conservative`/`conservative-feat`). |
| 12 | §G.5.5 | `GroupMember` is a two-variant type; group validation splits into a syntactic pass and a resolution pass. |
| 13 | §G.6.4 | `SeverityInference` is a `callisto-graph`-owned seam, and `CommitInference` lives here behind an optional `inference` feature. |
| 14 | §G.6.6 | An explicit `none` changeset entry is a naming event for linked joint detection; the union covers the named subset only. |
| 15 | §G.6.7 | A linked joint release forces a shared maximum **version**, not just a shared severity. |
| 16 | §G.7.1 | `mode = "always"` does not trigger peer escalation (invariant 9's out-of-range condition read literally), but **does** collapse every coverage answer into the bump branch, and attributes the resulting bump to `cascade.mode`. |
| 17 | §G.7.1 | `Coverage::Unknown` behaves as `Covers` under `out-of-range` and (via `always`'s blanket rule) `DoesNotCover` under `always`, and is never rewritten. |
| 18 | §G.7.5 | The fixed-group severity union is maintained by the fixpoint's `raise` operator, closing the peer-escalation hole in §7.1's pre-step-only claim. |
| 19 | §G.7.6 | The convergence bound is `4n + 1`, derived from the severity lattice's height. |
| 20 | §G.7.7 | Round-trip fidelity is verified by re-parse plus re-coverage, not asserted by the renderer; the grammar half is `callisto-manifests::round_trip`'s. |
| 21 | §G.8.3 | The aligned-base fallback for a fixed group with no released member. |
| 22 | §G.8.4 | napi drift compares target **triples** via a localized table, not derived package names. |
| 23 | §G.10.2 | §7.6's mutation ordering (`apply_version_plan`) lives in `callisto-graph`. |
| 24 | §G.11 | `commands::init` exists here, with the pinned signature. |
| 25 | §G.12 | `GraphError` gains transparent `Config`/`Changelog`/`Conventional` wrappers plus `WorkspaceVersionConflict`; `ConfigError` is declared here. |
| 26 | §G.5.1 | `[[fixed-group]]`/`[[linked-group]]` are a config layer, ranked between `[[package]]` and `[[package-set]]`, carrying per-package keys only (§14's `pre-major-inference` availability made resolvable). |
| 27 | §G.5.1 | `moon.yml`'s block also accepts `pre-major-inference` and `changelog`; it accepts no workspace-level key. |
| 28 | §G.7.3 | Dependency-spec writes carry a typed `DepWriteTarget`; `RewriteKey.kind` is `None` for a Cargo workspace-root target so inheriting members collapse to one rewrite. |
| 29 | §G.10.1 | `PlannedBump.writes` carries typed `VersionWriteTarget`s, disambiguating `[workspace.package].version` from a root package's own `[package].version`. |
| 30 | §G.10.2 | `ApplyOptions::transient` suppresses steps 7/8/11 for `snapshot`, which would otherwise delete every pending changeset (§8's "uncommitted, untagged"). |
| 31 | §G.11 | `pre.json`'s `initialVersions` is keyed by ecosystem-native name (npm-preferred for Case D), computed here, not by `PackageId::display_name`. |
| 32 | §G.11 | `npmPlatformPackages[]` and `depends_on_platforms` derive from `IdentityIndex::platforms_of`, since platform manifests are not `Package`s (M7). |
| 33 | §G.11 | The snapshot version is exactly `0.0.0-{tag}-{sha7}`, one value for the whole workspace. |
| 34 | §G.11 | `compose-pr-body` computes real prospective versions via `plan_version` instead of rendering `## Unreleased`. |
| 35 | §G.11 | Strictness escalation is one `escalate` call at the command boundary; `--strict`/`--strict-graph` exist on `status` and `version`, not only `validate`. |
| 36 | §G.7.4/§G.8.3 | `bump_target` delegates to `fixed_group_target` for every fixed-group member, which is what realises §7.5's new-member force-set. |
| 37 | §G.11 | `matrix` joins two independently-pinned tables per triple (`napi.rs`'s shared platform/arch/abi table, and a new 18-entry hostRunner/useCross table); a triple absent from either is excluded and diagnosed, never a hard error. |
| 38 | §G.11 | `matrix`'s `UnrecognisedPlatformTriple`/`DuplicatePlatformTriple` diagnostics carry no `escalated_by`, and `MatrixOptions` has no `--strict`/`--strict-graph` — a discovery report degrades gracefully rather than failing CI over one unregistered platform. |
| 39 | §G.11 | A package declaring both `engines.node` and `requires-python` orders the npm entry before the python entry in `runtimeVersions[]`, fixed for deterministic `--format json` output. |

### G.17 Deliberately not owned by this crate

| Concept | Owner | Why not here |
|---|---|---|
| `bump_version`, the `Versioning` trait | `callisto-format` | §6.2, decision doc's 0.x resolution. This crate decides *which* severity applies; that crate applies it, rigidly, with no config path reaching it. |
| `Changeset`, `pre.json` byte format | `callisto-format` | §6, §6.4 — the P1 adoption gate, with its own byte-compat corpus. |
| Conventional-commit parsing, `BREAKING CHANGE:` case-sensitivity | `callisto-conventional` | §13 inv. 11. This crate supplies the window and applies §14's `pre-major-inference` key mapping; it never reads a commit message. |
| `Manifest` read/write, format-preserving editing, `WorkspaceCargoResolver`, per-ecosystem spec grammars | `callisto-manifests` | §15, §17 v0.1. Feature-flagged per ecosystem; this crate names write *targets* and applies rewrite *policy*, not bytes and not grammar. |
| Changelog rendering | `callisto-changelog` | §7.6 step 7. This crate builds the `ChangelogInput`; that crate formats it. |
| `MoonProjectLocator`, the `DependencyScope → DeclaredEdgeKind` mapping | `callisto-moon` | §15, §13 inv. 26. This crate must not contain an "is moon available?" branch. |
| All plan/report value types, `PackageId`, `Version`, `DepSpec`, `Coverage`, `TagTemplate`, `CommandRunner`, `Diagnostic`, `ConfigKey` | `callisto-model` | §M — the MIT/Apache public-contract tier (§16, decision doc change 4). |
| `.callisto/plan.json` (§7.6 step 10) | `callisto-cli` | `.gitignore`d, never load-bearing (P2, §13 inv. 4), explicitly not the versioned contract. |
| argv parsing, rendering, `(default)` markers on attribution lines | `callisto-cli` | §M.11.1, §18 Q5.4. This crate computes the attribution and hands over the `ConfigKey` plus `ResolvedConfig::provenance`; the wrapper formats it (P6). |
| Publishing anything | the calling workflow | §9. Callisto is a versioning coordinator; `plan-publish` produces an order, never an execution. |

---

## 8. `callisto-cli`

**Purpose.** Argv parsing, rendering, and process I/O — nothing else.

**License:** AGPL-3.0 (§16, §15's crate table — the permissive tier is `callisto-format` and
`callisto-model` only). **Milestone:** v0.1 (`add`/`status`/`version`/`init`), v0.2 for the
rest (§17).

### CLI.0 Purpose, dependencies, and what makes invariant 27 structural

Per P6 and §13 invariant 27, this crate contains **no release semantics**: no bump computation,
no cascade, no graph construction, no manifest writing. Every subcommand handler is the
compute/apply split made literal at the call-site level (§0.1 rule 3): **construct inputs →
call one `callisto-graph`/`callisto-format` function → render the result.** Where a handler
needs more than one library call (e.g. `version` = `plan_version` + `apply_version_plan`), each
call is still a single, undecomposed library entry point — the handler never reimplements what
is between them.

| Edge | Kind | Why |
|---|---|---|
| `callisto-cli → callisto-model` | normal | report types, `CommandRunner`, `ConfigKey` |
| `callisto-cli → callisto-graph` | normal | `commands::*`, `Workspace`, `IgnoreWalkLocator`, `find_workspace_root`, `apply_version_plan`, the option/report types those need |
| `callisto-cli → callisto-format` | normal | `add` and `pre` construct changeset/`pre.json` values directly (§CLI.6.1, §CLI.6.4), since neither is exposed as a `callisto-graph` command function |
| `clap`, `clap_complete`, `serde_json`, `thiserror` | normal | argv, completions, JSON emit, error bridging |
| `callisto-fixtures` | **dev**, `graph` feature | §CLI.9 — the corpus, plus `ReplayCommandRunner` (§CF.3.4), which item 2's stdout-purity fixture needs to replace `CliCommandRunner` with |

**`callisto-cli` must not depend on `callisto-manifests`.** This is §13 invariant 27's
structural enforcement mechanism (§G.1.7 decision 4): manifest writes only ever happen inside
`apply_version_plan`, so a CLI that cannot reach `callisto-manifests` at all cannot, even
accidentally, grow a manifest-writing code path outside that one call. `callisto-graph`'s
cascade/groups/aggregation modules are additionally not `pub` beyond `commands::*` and the
option/report types those functions need — so invariant 27 is enforced twice, once by
dependency-graph absence (CI job, §CLI.9 item 5) and once by module privacy (compile-time).

**A `lib` target as well as a binary.** `callisto-moon` reuses this crate's clap definitions
and its pure renderers behind a `wrapper` feature (§CLI.8, §MO.2.3) so that the two wrappers'
flag grammars and human output can never independently drift. That reuse is the reason the
`render::*` functions below are generic over `io::Write` rather than writing to stdout
directly.

**What this crate deliberately does not own.** Everything under §M.15/§G.17/§CM.0.1's
"deliberately not owned" tables. Concretely: no `Package`/`DepEdge`/graph construction, no
manifest read/write (unreachable), no changelog rendering (`callisto-changelog`, called only
from inside `apply_version_plan`), no publish execution (§9 — nobody's job inside callisto).
What it does own: `clap` argument definitions, `--format json`/text dispatch, the real
`CommandRunner`/`ProjectLocator` selection that gives the core its I/O, `.callisto/plan.json`
(§7.6 step 10), and rendering — including the `(default)` marker on attribution lines (§M.11.1,
§18 Q5.4 mechanism 2), which requires resolved config in hand and is explicitly this crate's
job, not `callisto-graph`'s (P6).

### CLI.1 Crate layout

```
callisto-cli/
├── src/
│   ├── main.rs           # entry point: parse argv, dispatch, map Result to ExitCode
│   ├── lib.rs             # the `wrapper`-feature surface: `cli`, `render`, `output` re-exports
│   ├── cli.rs             # `Cli`, `GlobalArgs`, `Command` — the clap surface (§CLI.2)
│   ├── runner.rs          # `CliCommandRunner` (§CLI.3)
│   ├── workspace.rs       # `load_workspace()` — root discovery + `Workspace::load` wiring
│   ├── output.rs          # `OutputFormat`, `emit_report`, stdout/stderr discipline (§CLI.5)
│   ├── error.rs           # `CliError` (§CLI.7)
│   ├── render/
│   │   ├── mod.rs
│   │   ├── status.rs, version.rs, plan_publish.rs, snapshot.rs,
│   │   │   compose_pr_body.rs, validate.rs, tag.rs, init.rs   # one human renderer per
│   │   │                                                       # Report type (§CLI.5.2)
│   │   └── attribution.rs   # governedBy + BumpReason + (default) marker (§CLI.5.3)
│   └── commands/
│       ├── add.rs, status.rs, version.rs, pre.rs, validate.rs, snapshot.rs,
│       │   init.rs, plan_publish.rs, compose_pr_body.rs, tag.rs, completions.rs
│       └── mod.rs           # one thin handler fn per subcommand, matching §CLI.6
```

No `wasm` feature and no `wasm32-wasip1` target for this crate. §11's WASM build line and the
`wasm`/`cargo`/`npm` feature triad belong to `callisto-moon` (§MO.7) — `callisto-cli` is a
native binary (plus a `wrapper`-featured lib) only; the v0.1 `wasm32-wasip1` CI job (§13 inv.
26) exercises the core crates directly via `callisto-fixtures`, not through this crate, since
`callisto-moon` does not exist until v0.4 (§17) and the CI rule is explicit that it runs
"before `callisto-moon` exists."

### CLI.2 The clap surface

```rust
/// `callisto <subcommand> [args]`. Every field here is a GLOBAL flag — available before and
/// independent of the subcommand.
#[derive(clap::Parser)]
#[command(name = "callisto", version)]
pub struct Cli {
    #[command(flatten)]
    pub global: GlobalArgs,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(clap::Args, Clone, Debug)]
pub struct GlobalArgs {
    /// Output format for this invocation's primary payload. §13 invariant 5's contract is
    /// keyed on this flag's value, not on a heuristic (TTY detection, etc.) — the human/JSON
    /// choice must be explicit and deterministic for CI use.
    #[arg(long, global = true, value_enum, default_value = "text")]
    pub format: OutputFormat,

    /// Workspace directory to operate in. Defaults to the current directory; workspace *root*
    /// is then discovered upward from here (§CLI.4). Mirrors the Action's own `cwd` input
    /// (§12.3) so the same value threads through unmodified when the Action invokes the
    /// binary directly.
    #[arg(long, global = true, default_value = ".")]
    pub cwd: PathBuf,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputFormat { Text, Json }

#[derive(clap::Subcommand)]
pub enum Command {
    Add(AddArgs),
    Status(StatusArgs),
    Version(VersionArgs),
    Pre(PreArgs),
    Validate(ValidateArgs),
    Snapshot(SnapshotArgs),
    Init(InitArgs),
    PlanPublish(PlanPublishArgs),
    ComposePrBody(ComposePrBodyArgs),
    Tag(TagArgs),
    Completions(CompletionsArgs),
    Matrix(MatrixArgs),
}
```

> `[SPEC DECISION, not in 00-design.md: the exact global-flag surface (`--format`, `--cwd`).]`
> §12.5 establishes `--format json` as the contract-bearing flag, and the Action's `cwd` input
> (§12.3) implies the binary accepts an equivalent, but neither section gives the CLI its own
> flag names. `--format`/`--cwd` are the smallest names consistent with the Action's vocabulary
> (P7: the JSON contract, not the flag spelling, is what is versioned — §13 inv. 17 pins that
> renaming an *existing* flag is breaking, not the initial name) and with P4 (uniform global
> flags rather than one command inventing its own `--json`).

### CLI.3 `CliCommandRunner` — the real `CommandRunner` impl

```rust
/// Implements `callisto_model::CommandRunner` over `std::process::Command`. The CLI-side half
/// of §M.10's two-implementation trait (the other is `callisto-moon`'s, §MO.5).
pub struct CliCommandRunner;

impl CommandRunner for CliCommandRunner {
    fn run(&self, program: &str, args: &[&str], cwd: &Path)
        -> Result<CommandOutput, CommandError>
    {
        // Stdio::piped() for BOTH streams, unconditionally — never Stdio::inherit(). This is
        // what makes §13 invariant 5 hold structurally at the spawn site (§M.10's own doc
        // comment on `CommandRunner` names this exact requirement).
        let output = std::process::Command::new(program)
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output();
        match output {
            Ok(o) => {
                let out = CommandOutput {
                    exit_code: o.status.code(),
                    stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
                };
                if !out.stderr.is_empty() {
                    eprint!("{}", out.stderr);
                }
                Ok(out)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound =>
                Err(CommandError::NotFound { program: program.to_string() }),
            Err(e) => Err(CommandError::Io {
                program: program.to_string(), message: e.to_string() }),
        }
    }
}
```

> `[SPEC DECISION, not in 00-design.md: captured subprocess stderr is forwarded to callisto's
> own stderr unconditionally, at the runner level rather than per call site.]` §M.10 requires
> capture-not-inherit; §M.10's doc comment on `CommandOutput.stderr` says forwarding is what
> happens but not who does it or when. Doing it once, centrally, in `CliCommandRunner::run` —
> rather than in every `callisto-graph` call site that happens to invoke `git` — keeps
> `callisto-graph` free of any `--format`-awareness (it has none; P6 forbids a wrapper concern
> leaking into the core) and satisfies §13 invariant 5 with one implementation instead of N.
> Doing it unconditionally rather than only in JSON mode is the smaller rule and is harmless in
> text mode, where stderr was never part of any contract.

Git capability probing (P7's version half) is `callisto_graph::probe_git` (§G.2.4), called once
at process start before any subcommand handler runs, so a mismatch is a startup error rather
than a confusing failure mid-command. It is shared with `callisto-moon` rather than reimplemented
there.

### CLI.4 Workspace construction

```rust
/// Discovers the workspace root upward from `global.cwd` (§G.2.1), then builds a
/// `Workspace<CliCommandRunner, ManifestWalkResolver>` via `IgnoreWalkLocator`.
///
/// moon detection and `MoonProjectLocator` selection are `callisto-moon`'s job (§10, §MO.4),
/// never this crate's: `callisto-cli` always uses the `ignore`-walk discovery path, which is
/// correct even inside a moon workspace, since moon's structural advantage (catching a project
/// the manifest walk cannot see) is only reachable from inside moon's own host — a native CLI
/// run would have to shell out to `moon` itself, which §0.1's Option C explicitly declined to
/// build into the CLI.
pub fn load_workspace<'a>(
    global: &GlobalArgs,
    runner: &'a CliCommandRunner,
) -> Result<Workspace<'a, CliCommandRunner, ManifestWalkResolver>, CliError> {
    let start = dunce::canonicalize(&global.cwd)
        .map_err(|source| CliError::Io { source, path: Some(global.cwd.clone()) })?;
    let root = callisto_graph::locate::find_workspace_root(&start)?;
    let locator = callisto_graph::locate::IgnoreWalkLocator::new(&root);
    Ok(Workspace::load(root, &locator, runner)?)
}
```

> `[SPEC DECISION, not in 00-design.md: `find_workspace_root`'s signature and
> `IgnoreWalkLocator::new`'s constructor.]` §M.13.3 declares `LocateError::WorkspaceRootNotFound`
> and §15 names `IgnoreWalkLocator` as living in `callisto-graph`, but neither gives these two
> entry points a signature — something has to turn a starting directory into the root
> `Workspace::load` requires, and this is the smallest shape consistent with
> `ProjectLocator::projects(&self)` taking no path argument (so the root must already be baked
> into the locator at construction). §G.2.1/§G.2.2 pin them on the graph side.

### CLI.5 Rendering and the `--format json` contract

#### CLI.5.1 stdout/stderr discipline

```rust
/// The one function every subcommand handler's success path funnels through. §13 invariant 5,
/// restated at the render layer (the spawn-site half is §CLI.3's job): in `Json` mode this is
/// the ONLY thing this process ever writes to stdout, written exactly once.
pub fn emit_report<R: Report>(report: &R, format: OutputFormat) -> io::Result<()> {
    match format {
        OutputFormat::Json => write_json(&mut io::stdout(), report),
        OutputFormat::Text => render::human(report, &mut io::stdout()),
    }
}

/// 2-space pretty, trailing newline — matching §M.12.1's golden-file convention so a human
/// piping stdout to a file gets byte-identical output to the fixture corpus, not a
/// coincidentally-similar one. Generic over `Write` so `callisto-moon` can render into a
/// string for its host response (§MO.2.3).
pub fn write_json<W: io::Write, R: Report>(w: &mut W, report: &R) -> io::Result<()>;

/// Progress/log lines a handler wants to emit *before* the final report (e.g. `init`'s
/// discovery narration, §18 Q5.5). Always stderr in `Json` mode, so nothing but the report
/// itself reaches stdout; stdout in `Text` mode, since text output has no single-shot contract
/// to protect and interleaving progress with the human-readable result is the expected shape
/// for a terminal.
pub fn log_line(format: OutputFormat, line: &str) {
    match format {
        OutputFormat::Json => eprintln!("{line}"),
        OutputFormat::Text => println!("{line}"),
    }
}
```

> `[SPEC DECISION, not in 00-design.md: stdout JSON is 2-space pretty-printed with a trailing
> newline, rather than compact single-line JSON.]` §M.12.1 pins this formatting for **fixture
> golden files** and is explicit that "nothing pins stdout JSON's formatting." Matching the
> golden-file convention for real stdout too is the smallest decision that keeps `jq`-based
> consumers (§9.3, §12.2) working unchanged (`jq` does not care about whitespace) while making
> `callisto … --format json > out.json` diffable against a fixture byte-for-byte, which is
> useful for anyone debugging against the corpus.

#### CLI.5.2 Human rendering — one function per `Report` type

Each `render::<command>::human<W: Write>(report: &T, w: &mut W) -> io::Result<()>` is a pure
formatter: it prints exactly what the corresponding JSON's fields say, in prose, and calls
`render::attribution` (§CLI.5.3) wherever a `BumpRecord`/`Diagnostic` carries a `governed_by`.
No renderer recomputes anything — every value it prints came off the report struct, per P6.
Generic over `W` specifically so `callisto-moon` reuses the identical function to fill
`ExecuteExtensionOutput.rendered` (§MO.2.3) rather than growing a parallel renderer.

Representative shape (`version`'s):

```
  @myorg/cli   1.4.2 → 1.4.3   cascade: dep @myorg/sdk 1.2.0 → 2.0.0, out of range "^1.2"
                               governed by [cascade].bump-severity = "patch" (default)
  @myorg/host  3.1.0 → 4.0.0   cascade: peer dep @myorg/sdk bumped minor-or-worse
                               governed by [cascade].peer-escalation = true (default, §13 inv. 9)
  crates/tooling                spec rewrite only, no bump (dev-dep, §7.4)
```

— the literal §18 Q5.4 mechanism-2 example, produced by formatting `BumpRecord.reason`
(`BumpReason::Cascade`/`PeerEscalation`) and `BumpRecord.governed_by` against
`ResolvedConfig`'s provenance (§CLI.5.3).

#### CLI.5.3 Attribution rendering — the `(default)` marker

```rust
/// Renders one attribution line: `governed by {key} = {value} (default)` or
/// `governed by {key} = {value}` when explicit. `value` is read from the resolved config via
/// `key`, not carried in the `Diagnostic`/`BumpRecord` itself — §M.11.1 is explicit that only
/// the key travels in JSON; the value and the marker are this crate's to look up.
///
/// `{key}` is `ConfigKey::as_str()` **rewritten to §14's TOML-table syntax**, not printed
/// verbatim — `"cascade.bump-severity"` renders as `[cascade].bump-severity`, matching
/// §18 Q5.4's worked example: split at the first `.`, wrap the first segment in `[...]`,
/// keep everything after the first `.` as-is. A `ConfigKey` with no `.` at all
/// (`RELEASE_TRIGGER`, `PRE_MAJOR_INFERENCE`, `TAG_TEMPLATE`, `FIXED_GROUP`, `LINKED_GROUP` —
/// §M.11.1's package-scoped keys, which have no enclosing `[table]` in §14's syntax) renders
/// bare, with no brackets: `governed by release-trigger = "auto"`. This split-at-first-dot
/// rule is the one place the wire form and the human-readable form actually diverge, so it
/// is named here rather than left for an implementer to infer from the worked example alone.
pub fn attribution_line(key: &ConfigKey, cfg: &ResolvedConfig) -> String;
```

The provenance half is `callisto_graph::config::ResolvedConfig::provenance(&ConfigKey) ->
ConfigProvenance` and `rendered_value(&ConfigKey) -> Option<String>` (§G.5.4). An earlier draft
of this crate's spec introduced a local `ConfigProvenanceLookup` trait as a stand-in because
`ResolvedConfig`'s API was not yet pinned; §G.5.4 pins it, so the trait is dropped and the
concrete type is used. §11 records the reconciliation.

### CLI.6 Subcommands

Every handler has the shape
`fn handle(args: XArgs, global: &GlobalArgs) -> Result<ExitCode, CliError>` — omitted from each
signature below to avoid repetition. `runner`/`ws` construction (§CLI.3/§CLI.4) is implicit in
every handler that needs a workspace.

#### CLI.6.1 `add` — §6.1

```rust
#[derive(clap::Args)]
pub struct AddArgs {
    /// `name:severity`, repeatable. Non-interactive mode — the only mode available under
    /// `--format json` and the only one exercised by CI. §6.1's name grammar (bare or
    /// `ecosystem/name`) applies verbatim; resolution/ambiguity errors are §5.4's.
    #[arg(long = "package", value_name = "NAME:SEVERITY")]
    pub packages: Vec<String>,
    /// The changeset's summary paragraph. Required in non-interactive mode.
    #[arg(long)]
    pub summary: Option<String>,
}
```

Interactive mode (package picker + severity + summary editor) is the default when no
`--package` flags are given and stdin is a TTY. When stdin is not a TTY and no
`--package`/`--summary` were supplied, the handler returns `CliError::NotATty` rather than
hanging.

Wiring:

1. `load_workspace`, to validate and disambiguate names against `ws.graph.packages()` per
   §5.4's resolution order.
2. Build a `callisto_format::Changeset { entries, summary }`, where each `Entry::name` is the
   resolved `PackageId::display_name()` — §5.4's "on write, emit the shortest unambiguous
   form."
3. `callisto_format::write_changeset(&changeset)?` → a `String`.
4. Generate a filename and `fs::write` it into `ws.root.join(&ws.config.changesets_dir)`.
5. Print the written path (`Text`) or the §CLI.6.12 envelope
   `{"schemaVersion": 1, "command": "add", "path": "…"}` (`Json`).

> `[SPEC DECISION, not in 00-design.md: `add` has no §12.5 report type, and the changeset
> file's *name* is generated by this crate, not by `callisto-format`.]` §12.5's per-command
> JSON list does not include `add` — its output is not part of the versioned contract, only its
> on-disk side effect is (byte-compat per P1) — so its JSON output is an ad-hoc envelope, not a
> fixtured contract. Filename generation lands here because §6.1 says "filenames arbitrary" and
> §F.3 keeps `callisto-format` filesystem-free, so the only crate that can own it is the one
> doing the write. The generated name follows `@changesets/cli`'s convention (a
> human-readable three-word slug, e.g. `brave-pandas-shave.md`) purely for familiarity; nothing
> depends on its shape, and a collision is resolved by regenerating. An earlier draft of this
> crate's spec invented a `callisto_format::changeset::write(&dir, &entries, &summary)` that
> took a directory path — incompatible with §F.3; §11 records the reconciliation.

#### CLI.6.2 `status` — §12.5

```rust
#[derive(clap::Args)]
pub struct StatusArgs {
    /// Escalates the warn-by-default cross-checks `status` surfaces — §7.5's napi drift and,
    /// once §6.3 ships (v0.2), empty-changeset validation — to hard failures (exit 1).
    #[arg(long)]
    pub strict: bool,
    /// Escalates §7.2's moon edge cross-check. `status` builds the graph, so the cross-check
    /// runs here too (§G.4.6); §18 Q5.4 mechanism 1 is explicit that between `init` runs,
    /// "`status`/`version` surface the same drift as warn-by-default cross-checks,
    /// `--strict`-escalatable."
    #[arg(long)]
    pub strict_graph: bool,
}
```

`load_workspace` →
`callisto_graph::commands::status(&ws, &StatusOptions { strict, strict_graph })` →
`emit_report`. `status` never writes anything under any flag; the flags change only whether a
`Diagnostic` is reported as `Warning` or `Error`, and therefore whether the process exits `0`
or `1` (§CLI.7).

> `[SPEC DECISION, not in 00-design.md: `--strict`/`--strict-graph` exist on `version` and
> `status`, not only on `validate`.]` An earlier draft put `--strict-graph` on `validate`
> alone, which cannot work: the moon edge cross-check runs inside `ManifestWalkResolver::build`
> (§G.4.1 step 8), so it fires on **every** command that constructs a graph — `status` and
> `version` included — and a diagnostic that can be emitted but not escalated is a diagnostic
> whose `escalated_by: Some(StrictFlag::StrictGraph)` field (§M.11.2) names a flag the user
> cannot pass. §18 Q5.4 mechanism 1 and §6.3's `--strict` composition paragraph both describe
> the flags as per-check and independently composable, not as one command's local options.

#### CLI.6.3 `version` — §7.6, §12.5

```rust
#[derive(clap::Args)]
pub struct VersionArgs {
    /// §7.6 step 9 — off by default, can be slow at scale.
    #[arg(long)]
    pub refresh_lockfiles: bool,
    /// Escalates §6.3's empty-changeset validation (v0.2+) and §7.5's napi drift check to hard
    /// failures.
    #[arg(long)]
    pub strict: bool,
    /// Escalates §7.2's moon edge cross-check to a hard failure. Separately named from
    /// `--strict` because that check is itself opt-in (not every workspace runs under moon),
    /// so §6.3 requires it to be escalatable independently; the two compose freely and
    /// neither implies the other. Present on `version` — not only on `validate` — because the
    /// cross-check runs during **graph construction** (§G.4.6), which `version` performs on
    /// every invocation, and §18 Q5.4 mechanism 1 requires `status`/`version` to surface that
    /// drift `--strict`-escalatably.
    #[arg(long)]
    pub strict_graph: bool,
    /// §6.3's escape hatch, per-invocation: "escape hatch via `--allow-empty-changesets` **or**
    /// `[validation].allow-empty-changesets = true`". The flag ORs with the config key rather
    /// than overriding it, since both spellings mean "permit it this time" and neither has a
    /// reason to turn the other off. For an intentional re-release — a security-only re-tag
    /// with no code diff — where editing committed config for one run would be worse.
    #[arg(long)]
    pub allow_empty_changesets: bool,
}
```

```rust
let ws = load_workspace(&global, &runner)?;
let inference = NoInference;      // hardcoded as of this writing -- see §CLI.6.3.1
let plan = callisto_graph::commands::plan_version(&ws, &inference, &VersionOptions {
    strict: args.strict,
    strict_graph: args.strict_graph,
    allow_empty_changesets: args.allow_empty_changesets,
})?;
let outcome = callisto_graph::apply_version_plan(
    &ws.root, &plan, &runner,
    &ApplyOptions { refresh_lockfiles: args.refresh_lockfiles, transient: false })?;
write_plan_json(&ws.root, &plan)?;                      // §7.6 step 10, .gitignore'd
let report = plan.to_report(outcome.lockfile_refresh_results);
emit_report(&report, global.format)
```

**CLI.6.3.1 — `SeverityInference` selection, *intended* to be milestone-gated by Cargo feature:**

This was the plan; it has not shipped. `callisto-cli` does forward an `inference` Cargo
feature to `callisto-graph/inference` (`Cargo.toml`), but no command handler consults it —
`commands/version.rs` and `commands/compose_pr_body.rs` both hardcode `let inference =
NoInference;` unconditionally, with no `#[cfg(feature = "inference")]` branch anywhere in
either file. Enabling `--features inference` on `callisto-cli` today compiles
`callisto_graph::infer::CommitInference` into the dependency tree but nothing ever selects it,
so it has no effect on the binary's behavior. The `select_inference` function below never
existed in real code; it describes the intended shape, not a call site that was removed:

```rust
// INTENDED (§17, §G.14) -- not implemented in callisto-cli as of this writing.
#[cfg(not(feature = "inference"))]
fn select_inference(_: &Workspace<'_, …>) -> impl SeverityInference {
    callisto_graph::infer::NoInference
}

#[cfg(feature = "inference")]
fn select_inference<'a>(ws: &'a Workspace<'a, …>) -> impl SeverityInference + 'a {
    callisto_graph::infer::CommitInference
}
```

> `[SPEC DECISION, not in 00-design.md: the concrete `SeverityInference` impl is selected
> behind a Cargo feature (`inference`) rather than a runtime branch.]` §17 v0.1 ships
> `callisto-graph` with `infer::NoInference` and defers "`infer` wired to
> `callisto-conventional`" to v0.2 explicitly — a v0.1 binary that unconditionally linked the
> inference path would ship dead code for a milestone. The feature flag is the smallest
> mechanism consistent with P4's bounded-work promise: v0.2's binary flips the default-features
> list, no handler code changes. The impl itself is `callisto-graph`'s, not this crate's
> (§G.6.4) — an earlier draft placed it in `callisto-conventional`, which cannot see the trait;
> §11 records the reconciliation. As the paragraph above notes, this decision was made but the
> handler-side wiring it describes was never actually implemented.

#### CLI.6.4 `pre` — §8

```rust
#[derive(clap::Subcommand)]
pub enum PreArgs {
    Enter { tag: String },
    Exit,
}
```

**`Enter`**: `load_workspace` → `ws.initial_versions()?` (§G.11) →
`PreState::entering(tag, snapshot)` (§F.7) → `callisto_format::write_pre_json` → `fs::write` to
`ws.root.join(&ws.config.changesets_dir).join("pre.json")`.

The snapshot comes from `Workspace::initial_versions`, **not** from anything this crate reads
or keys itself, for two structural reasons this handler cannot work around: `Package` has no
version field (§M.6.1's SPEC DECISION M6) and this crate cannot depend on `callisto-manifests`
(§G.1.7's SPEC DECISION G4), so the on-disk read has to happen in `callisto-graph`; and the
`initialVersions` **key** is the ecosystem-native package name, not
`PackageId::display_name()` — a `"cargo/foo"`-shaped key would break P1's byte-compat promise
for `pre.json` on exactly the cross-ecosystem workspaces that produce prefixed identities.
§G.11 pins both the accessor and the key rule.

`tag` is validated here before anything is written: it must be a legal SemVer prerelease
identifier and not purely numeric (`[0-9A-Za-z-]+`, at least one non-digit), because §F.6.3
composes it as `{release}-{tag}.{counter}` — a rejected tag is `CliError` with a message
naming the rule, never a `pre.json` that later makes every bump fail.

**`Exit`**: needs only the root, not the full graph — `find_workspace_root` alone, no
`Workspace::load` → read `pre.json` → `parse_pre_json` → `PreState::exiting()` →
`write_pre_json` → `fs::write`.

Neither has a §12.5 `Report` shape; both print a one-line confirmation (`Text`) or a minimal
JSON envelope (`Json`) — see §CLI.6.12 for that envelope, which carries `schemaVersion` even
though it is not a fixtured contract.

> `[SPEC DECISION, not in 00-design.md: `pre exit` deliberately skips full
> `Workspace::load`.]` §8 specifies `pre.json`'s semantics exhaustively but assigns no crate a
> Rust API; §F.7 and §M.15 put `PreState` in `callisto-format`. `exit` only flips `mode` without
> touching `initialVersions` (§8), so building a graph it does not read would be pure cost —
> cheap per P3, and consistent with `pre enter`'s snapshot need being the exception rather than
> the rule. An earlier draft gave `PreState` filesystem-touching `enter`/`exit` constructors;
> §F.7's pure `entering`/`exiting` plus this crate's own file I/O is the reconciliation (§11).

#### CLI.6.5 `validate` — §17 v0.2, §18 Q3

```rust
#[derive(clap::Args)]
pub struct ValidateArgs {
    #[arg(long)] pub staged: bool,
    #[arg(long, value_name = "REF")] pub since: Option<String>,
    #[arg(long)] pub strict: bool,
    #[arg(long)] pub strict_graph: bool,
}
```

`load_workspace` → `callisto_graph::commands::validate(&ws, &ValidateOptions { … })` →
`emit_report`. Exit code: `1` when `!report.ok`, `0` otherwise (§CLI.7).

#### CLI.6.6 `snapshot` — §8, §12.5

```rust
#[derive(clap::Args)]
pub struct SnapshotArgs {
    #[arg(long)] pub tag: String,
}
```

```rust
let ws = load_workspace(&global, &runner)?;
let (plan, report) = callisto_graph::commands::plan_snapshot(&ws, &args.tag)?;
callisto_graph::apply_version_plan(
    &ws.root, &plan, &runner,
    &ApplyOptions { refresh_lockfiles: false, transient: true })?;
emit_report(&report, global.format)
```

`plan_snapshot` itself hard-errors on `pre.json` mode `"pre"` (§8, §G.11) — the handler does
not duplicate that check. `refresh_lockfiles` is unconditionally `false` here: snapshot writes
are transient and uncommitted by design (§8), and a lockfile refresh would be a needless write
against files nobody commits.

**`transient: true` is not optional here.** Without it, `apply_version_plan` would run §7.6
steps 7, 8 and 11 — prepending a `## 0.0.0-<tag>-<sha>` section to every `CHANGELOG.md`,
**deleting every pending changeset file**, and staging the lot — which would destroy the
release intent the next real `version` run reads, in a command §8 describes as writing
"uncommitted, untagged." See §G.10.2's SPEC DECISION for the flag's full contract.

#### CLI.6.7 `init` — §5.3, §18 Q4/Q5.5

```rust
#[derive(clap::Args)]
pub struct InitArgs {
    /// Non-interactive: applies every offered diff without prompting. Required under
    /// `--format json`, for the same reason `add` requires non-interactive flags — and the
    /// mode moon's `initialize_extension` host surface drives (§10, §MO.2.4).
    #[arg(long)] pub yes: bool,
}
```

`load_workspace` → `callisto_graph::commands::init(&ws, &InitOptions { yes: args.yes })` →
`emit_report`. The "Discovery / Packages / Structure" narration from §Q5.5's worked example is
emitted via `log_line` during the call, before the final `InitReport` — `init`'s progress
output and its report are two different things, exactly as `version`'s attribution lines are
prose alongside, not inside, the report.

#### CLI.6.8 `plan-publish` — §9.2, §12.5

```rust
#[derive(clap::Args)]
pub struct PlanPublishArgs {}
```

`load_workspace` → `callisto_graph::commands::plan_publish(&ws, &PublishOptions::default())` →
`emit_report`. No CLI-level options: §9.2's plan shape is fully determined by on-disk state and
config; nothing about it is CLI-tunable.

#### CLI.6.9 `compose-pr-body` — §12.2, §12.5, §13 inv. 23

```rust
#[derive(clap::Args)]
pub struct ComposePrBodyArgs {
    /// The prior run's PR body, for `managedLabels` round-trip (§12.5's rule: "callisto's own
    /// last-known-applied set, round-tripped from the previous run's output via the existing
    /// PR body"). `-` reads stdin — the shape a calling workflow's `gh pr view --json body`
    /// piped straight in would use. Absent on a PR's first run: a routine state, not an error.
    #[arg(long, value_name = "TEXT|-")]
    pub existing_body: Option<String>,
    /// Repeatable — mirrors the Action's `labels` input (§12.3), passed straight through.
    #[arg(long = "label")]
    pub labels: Vec<String>,
}
```

`load_workspace` → `NoInference` (hardcoded, same as `version`; see §CLI.6.3.1) →
resolve `existing_body` (read stdin when it is `-`, else pass through) →
`callisto_graph::commands::compose_pr_body(&ws, &inference, &PrBodyOptions { … })` →
`emit_report`. The handler never calls `apply_version_plan` and never receives a `VersionPlan`
— §13 invariant 23 is about not depending on `version` having *run* (which would mean reading
files `version` deletes), not about avoiding the pure `plan_version` computation
`compose_pr_body` performs internally to fill in prospective versions (§G.11).

> `[SPEC DECISION, not in 00-design.md: `--existing-body` / `--label` as `compose-pr-body`'s
> input surface.]` §12.5 specifies the round-trip *semantics* precisely but not how the previous
> body reaches the CLI invocation, and §12.3 lists `labels` as an Action input without saying
> how the Action passes it to the binary. Reading the prior body from a flag (with `-` for
> stdin) rather than making a `gh` call inside this crate keeps `callisto-cli` free of
> GitHub-API awareness — the calling workflow already has the PR body (§12.2's flow assumes a
> PR exists to look up), so handing it in is cheaper and keeps this crate's process I/O surface
> to `git` and `fs` only (§9.5).

#### CLI.6.10 `tag` — §9.1, §17 v0.2, §13 inv. 16/24

```rust
#[derive(clap::Args)]
pub struct TagArgs {
    /// `plan-publish --format json`'s output. `-` reads stdin — the natural shape for
    /// `callisto plan-publish --format json | callisto tag --plan -`, mirroring §9.3's
    /// `jq`-piped worked example but through the sanctioned `tag` subcommand instead of raw
    /// `git tag` (§17 v0.2: "the sole sanctioned tag-creation path once tag ownership was
    /// resolved").
    #[arg(long, value_name = "FILE|-")]
    pub plan: String,
}
```

`load_workspace` → read/deserialize `PublishPlan` from `args.plan` →
`callisto_graph::commands::create_tags(&ws, &plan)` → `emit_report`. Creates local tags only
(§9.1); this handler never invokes `git push`, and nothing in this crate has a code path that
pushes anything, anywhere (§13 inv. 16).

> `[SPEC DECISION, not in 00-design.md: `--plan <FILE|->` is `tag`'s input mechanism.]` §17
> v0.2 commits to `callisto tag` existing and §9.1 fixes what it must do (create local tags
> from a plan's `releases[]`), but no section gives the subcommand an argument surface — §9.3's
> worked example predates `callisto tag`'s existence and shows raw `git tag` from `jq`
> instead. Taking the plan as a file/stdin argument, rather than re-deriving it by calling
> `plan-publish` internally, keeps `tag` a pure consumer of the one versioned JSON contract
> (§0.1 rule 4) instead of a second production path for `PublishPlan` values.

#### CLI.6.11 `completions` — CLI-only (§11)

```rust
#[derive(clap::Args)]
pub struct CompletionsArgs {
    #[arg(value_enum)] pub shell: clap_complete::Shell,
}
```

`clap_complete::generate(args.shell, &mut Cli::command(), "callisto", &mut io::stdout())`. No
workspace, no library call, no `Report` — genuinely CLI-only glue, the one command §11's table
marks ✗ for WASM (needs shell-integration machinery WASM has no use for; the decision doc names
this as the sole surviving CLI-only capability after `tag` and `init` both resolved
WASM-compatible).

#### CLI.6.12 `add` and `pre`'s ad-hoc JSON envelopes

> `[SPEC DECISION, not in 00-design.md: `add` and `pre` have no `Report` impl, but their
> `--format json` envelopes still carry `schemaVersion`.]` §13 invariant 14 is unqualified —
> "Schema version is a first-class, versioned contract for **every** `--format json`
> output" — while §12.5 enumerates per-command shapes for `status`, `version`, `plan-publish`,
> `snapshot`, and `compose-pr-body` only. Those are reconcilable in exactly one direction:
> `add` and `pre` are exempt from being *fixtured contracts* (§M.12.1 says why — their
> contract-bearing effect is on disk, per P1, not on stdout), but nothing makes them exempt
> from carrying the field, and a consumer that has to know *which* commands carry
> `schemaVersion` before it can safely read one has lost the property invariant 14 exists to
> give it. Carrying it costs one key.

```json
{ "schemaVersion": 1, "command": "add", "path": ".changeset/brave-pandas-shave.md" }
{ "schemaVersion": 1, "command": "pre", "mode": "pre",  "tag": "next" }
{ "schemaVersion": 1, "command": "pre", "mode": "exit", "tag": "next" }
```

`schemaVersion` is the same `callisto_model::SCHEMA_VERSION` constant every `Report` uses
(§M.12.1), bumped with it, so these envelopes cannot drift to a different number. `command`
mirrors `Report::COMMAND`'s role for a consumer that multiplexes several invocations' output.
What these envelopes do *not* get is a §12.6 golden file: they are unfixtured by the same
decision that keeps them out of §12.5, and a future edit is free to add fields to them without
that being a contract change.

#### CLI.6.13 `matrix` — §19, §G.11, §M.12.7

```rust
#[derive(clap::Args, Clone, Debug, Default)]
pub struct MatrixArgs {
    /// Restrict output to one registered package's name (`PackageId::name()`). An unknown name
    /// is `Err(GraphError::UnknownPackage)`, checked before any manifest is read (AC-007).
    #[arg(long)]
    pub package: Option<String>,
}
```

`load_workspace` → `callisto_graph::commands::matrix(&ws, &MatrixOptions { package: args.package.clone() })`
→ `emit_report`. No `--strict`/`--strict-graph`: §G.11's SPEC DECISION 38 makes this deliberate,
not an oversight — `matrix` is a read-only discovery report, not a release-gating command, and
its diagnostics are unconditionally `Warning`. `--format text` and the bare invocation (no flags
at all, since `text` is `GlobalArgs::format`'s default) render identically (AC-003b/AC-008).

Two failure modes are worth distinguishing for a caller: an unrecognised platform triple or a
duplicate triple within one package's declaration never fails the command (§G.11) — the package
still contributes whatever it validly declares, plus a diagnostic; a package declaring platform
targets via *both* `napi.targets` and `[tool.maturin].targets`, or a manifest that fails to parse
at all, is `Err` (AC-017, AC-010/010b/010c) — there is no partial matrix to fall back to once a
source manifest itself is unreadable.

### CLI.7 Errors and exit codes

```rust
/// Bridges every library error this crate can receive into one exit-code-mapped type. Never
/// itself carries release semantics — it is a rendering/dispatch concern (P6).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CliError {
    #[error(transparent)] Graph(#[from] GraphError),
    #[error(transparent)] Locate(#[from] LocateError),
    #[error(transparent)] Config(#[from] ConfigError),
    #[error(transparent)] Command(#[from] CommandError),
    #[error(transparent)] ChangesetParse(#[from] callisto_format::ParseError),
    #[error(transparent)] ChangesetWrite(#[from] callisto_format::WriteError),
    #[error(transparent)] PreJson(#[from] callisto_format::PreJsonError),
    #[error(transparent)] Io(#[from] std::io::Error),
    #[error("refusing to prompt interactively: stdin is not a terminal and no non-interactive \
             flags were given")]
    NotATty,
}
```

`callisto-format`'s error enums are named concretely here rather than boxed as strings; §F.5.5
and §F.7 pin their shapes, so there is no unspecified type to hedge against.

**Exit codes**, uniform across every subcommand:

| Condition | Exit code |
|---|---|
| Success | `0` |
| `validate` ran and `report.ok == false` (§12.5's `.ok` gate) | `1` |
| Any other command whose report's `.diagnostics` contains a `severity: Error` entry, after `escalate()` (§G.11) has run | `1` |
| Any `CliError` (library error, I/O failure) | `1` |
| `clap` argument-parsing failure (unknown flag, missing required value) | `2` (clap's own default) |

The second and third rows are the same check in substance — `validate`'s `.ok` field is a
convenience precomputed from exactly this rule (`ok == !diagnostics.iter().any(|d| d.severity
== Error)`), added because `validate`'s whole purpose is that check, not because status/version
are exempt from it. Every command that can carry an `escalated_by`-flagged diagnostic
(`status`, `version`, `validate` — per §CLI.10 #12's `--strict`/`--strict-graph` additions)
runs `escalate()` before rendering, so this row is what makes those flags actually change the
exit code rather than only the JSON output, closing the gap §CLI.6.2 already promised.

On error, the message (`CliError`'s `Display`, via `thiserror`) is printed to **stderr always**,
never stdout — even under `--format json`.

> `[SPEC DECISION, not in 00-design.md: errors render to stderr only, with no JSON error
> envelope, even under `--format json`.]` §13 invariant 5 pins what may reach stdout in JSON
> mode but is silent on what happens when a command fails before producing a report at all.
> Inventing an error-JSON shape would be a second, unfixtured contract surface (§12.6 fixtures
> the success shapes only); omitting it and relying on the exit code is the smaller schema
> surface and matches how every `jq`-based consumer in §9.3/§12.2 already has to check `$?`
> before piping stdout onward. A `--format json` invocation that fails therefore produces empty
> stdout and a nonzero exit code, which is the correct signal.

### CLI.8 Feature flags

```toml
# callisto-cli/Cargo.toml (representative)
[features]
default = ["cargo", "npm"]
cargo = ["callisto-graph/cargo"]     # forwards to callisto-manifests via callisto-graph
npm = ["callisto-graph/npm"]
# §CLI.6.3.1 — on by default from v0.2 onward; off in the v0.1 binary, which has no
# callisto-conventional in its dependency tree at all.
inference = ["callisto-graph/inference"]
# §MO.2.3/§MO.7 — the lib-only surface callisto-moon consumes: the clap argument definitions
# and the pure `render::*`/`write_json` functions, with `main`, the `commands/` handlers, the
# process-I/O runner, and `completions` all excluded. Keeps callisto-cli's binary-only code
# unreachable from the WASM build.
wrapper = []

[dependencies]
callisto-model = { path = "../callisto-model" }
callisto-graph = { path = "../callisto-graph" }
callisto-format = { path = "../callisto-format" }
clap = { version = "…", features = ["derive"] }
clap_complete = "…"
serde_json = "…"
thiserror = "…"
```

No `[features].wasm` — this crate never builds for `wasm32-wasip1` as a binary (§CLI.1); its
`wrapper`-featured lib does, as a dependency of `callisto-moon` (§MO.7). `cargo`/`npm` exist
here only to forward into `callisto-graph`'s identically-named features, which forward again
into `callisto-manifests`'s (§CM.8) — Cargo's additive-feature unification means a default
`cargo build` gets both ecosystems, matching v0.1's committed scope (§17), with no per-crate
feature bookkeeping beyond the forward.

### CLI.9 Fixture obligations

Per §12.6, for this crate specifically:

1. **CLI subcommand/flag corpus.** One fixture asserting the full set of subcommand names and
   their flag names (`clap`'s own `Command::get_subcommands()`/`get_arguments()`, snapshotted) —
   renaming any of them is a breaking change per §13 invariant 17 and must fail a fixture, not
   a changelog note.
2. **`--format json` stdout purity.** For every JSON-capable subcommand, run against a fixture
   workspace with `CliCommandRunner` replaced by a canned-output test double, and assert stdout
   is *exactly* one 2-space-pretty JSON value plus trailing newline — no log line, no progress
   narration, nothing from a captured subprocess's stderr — proving §13 invariant 5 end-to-end
   through this crate's actual `main()`, not just at the trait level (§M.10's and §CLI.3's
   fixtures prove the seam; this fixture proves the wiring).
3. **Attribution-line rendering.** A fixture `BumpRecord` with `governed_by = Some(..)` and a
   `BumpReason` variant, rendered through `render::attribution` against both
   `ConfigProvenance::Default` and `::Explicit`, asserting the exact §18 Q5.4 mechanism-2 line
   shape (`governed by {key} = {value}` vs. `… (default)`).
4. **Exit-code corpus.** One fixture per row of §CLI.7's table, including `validate`'s
   `ok: false` → `1` case specifically, since it is the one exit code this crate computes from
   report content rather than from an `Err`.
5. **`callisto-cli` dependency-graph audit.** A CI job (not a fixture, but co-located with the
   `xtask dep-audit` §G.1.7 already requires) asserting via `cargo metadata`'s resolve graph
   that `callisto-manifests` and `callisto-moon` are not **direct** dependencies of
   `callisto-cli` — the structural half of §13 invariant 27.

   **Direct, not transitive**, and the distinction is not a weakening: `callisto-cli →
   callisto-graph → callisto-manifests` is a real, required, deliberate chain (§G.1.5,
   §CLI.0), so a transitive assertion would be unsatisfiable by construction — it would fail
   on the very first commit that wires the CLI to the core, and the only way to make it pass
   would be to break the design. Direct non-dependency is also the semantically correct check:
   what invariant 27 protects is that `callisto-cli/src` cannot *name* `Manifest` or open a
   manifest, and a crate cannot name a type from a crate it does not directly depend on —
   Rust has no transitive-import path. §G.1.7's own statement of the same rule is already
   phrased this way ("the same `xtask` asserts that `callisto-cli`'s **manifest** does not
   depend on `callisto-manifests`"); this item is the one that had drifted.

   `callisto-moon` stays a **transitive** assertion, because there the chain genuinely must
   not exist in any form — the edge runs the other way (`callisto-moon → callisto-cli`,
   §MO.1), so any path from `callisto-cli` to `callisto-moon` would be a dependency cycle or a
   layering inversion, not a legitimate reuse.
6. **`add` round-trip.** `add --package foo:minor --summary "…"` writes a file that
   `callisto_format::parse_changeset` reads back to the identical `Changeset`, and whose
   frontmatter name is the shortest unambiguous form for the fixture workspace's identity set
   (§5.4).

These live in `callisto-cli/tests/`, dev-depending on `callisto-fixtures` for the corpus, rather
than inside `callisto-fixtures` itself — see §CF.2 for the cycle-avoidance rule.

### CLI.10 Index of `[SPEC DECISION]` flags

| # | Section | Decision |
|---|---|---|
| 1 | §CLI.2 | Global flag surface: `--format text\|json` (default text), `--cwd` (default `.`). |
| 2 | §CLI.3 | Captured subprocess stderr is forwarded to callisto's own stderr unconditionally, centrally in `CliCommandRunner`. |
| 3 | §CLI.4 | `find_workspace_root`/`IgnoreWalkLocator::new` signatures, filling a gap left by §M.13.3 naming the error but not the entry point. |
| 4 | §CLI.5.1 | Stdout JSON is 2-space pretty-printed with a trailing newline, matching the golden-file convention. |
| 5 | §CLI.6.1 | `add` has no §12.5 report shape; changeset **filename** generation lives here, not in `callisto-format`. |
| 6 | §CLI.6.3.1 | `SeverityInference` selection is Cargo-feature-gated (`inference`), tracking the v0.1/v0.2 milestone split without a runtime branch. |
| 7 | §CLI.6.4 | `pre exit` skips full `Workspace::load`; `PreState`'s pure constructors plus this crate's file I/O replace filesystem-touching constructors. |
| 8 | §CLI.6.9 | `compose-pr-body`'s prior-body/labels input surface (`--existing-body`, `--label`). |
| 9 | §CLI.6.10 | `tag`'s plan-input surface (`--plan <FILE\|->`). |
| 10 | §CLI.7 | Errors render to stderr only; no JSON error envelope, even under `--format json`. |
| 11 | §CLI.8 | A `wrapper` feature exposes argv definitions and pure renderers as a lib for `callisto-moon`, keeping binary-only code out of the WASM build. |
| 12 | §CLI.6.2 | `--strict`/`--strict-graph` exist on `status` and `version`, not only on `validate` — the moon cross-check runs during graph construction, which all three perform. |
| 13 | §CLI.6.3 | `--allow-empty-changesets` exists as a flag, OR-ed with `[validation].allow-empty-changesets` (§6.3 names both spellings). |
| 14 | §CLI.6.12 | `add` and `pre`'s ad-hoc JSON envelopes carry `schemaVersion` (§13 inv. 14 is unqualified) while staying unfixtured (§12.5 does not list them). |

### CLI.11 Deliberately not owned by this crate

| Concept | Owner | Why not here |
|---|---|---|
| Bump computation, cascade, groups, aggregation, mutation ordering | `callisto-graph` | P6, §13 inv. 27 — this crate calls `commands::*`/`apply_version_plan`, never reimplements what is behind them. |
| Manifest read/write | `callisto-manifests` | Unreachable from this crate at all (§CLI.0) — the structural enforcement of invariant 27. |
| Changeset/`pre.json` byte format | `callisto-format` | §6, §6.4 — this crate constructs and serializes values for `add`/`pre`, and does the file write, but never parses or emits the format itself. |
| Conventional-commit parsing | `callisto-conventional` | §7.1 — this crate selects a feature-gated impl (§CLI.6.3.1), never parses a commit message. |
| Changelog rendering | `callisto-changelog` | Called only from inside `apply_version_plan`; this crate never touches it directly. |
| moon extension host wiring | `callisto-moon` | §10, §15 — a structurally separate consumer of the same core. |
| Publishing anything | the calling workflow | §9 — `plan-publish`/`tag` produce data and local tags; nothing here shells out to `cargo publish`/`npm publish`. |
| The GitHub Action's orchestration (mode dispatch, PR creation, label application) | `orin-dx/callisto-action` | §12 — the Action is a CLI consumer (§0.1), composed in bash, not code in this crate. |

---

## 9. `callisto-moon`

**Purpose.** The WASM extension binary moon loads to run callisto as
`moon ext callisto -- <args>` — argv/host-call translation, `MoonProjectLocator`,
`MoonCommandRunner`, and nothing else.

**License:** AGPL-3.0 (§16 — this crate contains no primitive worth spreading independently;
it is glue between the coordination core and one specific, versioned host API).
**Milestone:** v0.4 (§17).

### MO.0 Purpose, posture, and the moon compatibility range

`callisto-moon` is, deliberately, the mirror image of `callisto-cli`: both are thin consumers
of the moon-agnostic core across the same enumerated seams — `Manifest`, `ProjectLocator`,
`DependencyResolver`, `CommandRunner` (§15) — with argv/host-call parsing, rendering, and
process/host I/O as their only local logic, exactly as §13 invariant 27 requires of
`callisto-cli`. Nothing that computes a bump, resolves a cascade, or decides publish order
lives here (P6).

**No independent API-stability promise (§0.1, §15).** This crate's own trait implementations —
above all `MoonProjectLocator` — are explicitly *not* the versioned contract. §0.1 rule 4 names
`--format json` on stdout as the stable surface; `callisto-moon` does not even have a stdout in
the conventional sense (its output is the extism host's response payload), so the only
externally-stable thing this crate produces is the *same* JSON report types (§M.12)
`callisto-cli` produces, serialized into the host response instead of printed.
`MoonProjectLocator`, the `DependencyScope → DeclaredEdgeKind` mapping (§MO.4.4), and
`MoonCommandRunner` are expected to break across moon minor versions the same way Nx's
`VersionActions` did inside one minor (decision doc, change 2) — that expectation is why this
crate pins a compatibility range rather than trying to track moon's `main` losslessly.

> `[SPEC DECISION, not in 00-design.md: the pinned moon compatibility range is
> `>=1.30.0, <2.0.0`, checked once per invocation against moon's reported version and enforced
> via `LocateError::IncompatibleMoonVersion` (§M.13.3).]` §15 commits to "a specific moon
> compatibility range" without naming one, since pinning a real version depends on which moon
> release first stabilized the extension API surface this crate targets
> (`register_extension`/`define_extension_config`/`execute_extension`/`initialize_extension`,
> plus `exec_command`/`host_log`/`from_virtual_path`/`to_virtual_path` as host functions). The
> number itself is a placeholder to be corrected against moon's actual changelog at
> implementation time — what is load-bearing is that it is a *checked* range, not a floating
> assumption, and that the check happens exactly once per invocation (P2/P3: cheap, no
> redundant host round-trips) rather than per host-function call.

### MO.1 Dependencies

| Edge | Kind | Why |
|---|---|---|
| `callisto-moon → callisto-model` | normal | report types, `ProjectRoot`, `DeclaredEdge`, `CommandRunner` |
| `callisto-moon → callisto-graph` | normal | `ProjectLocator`/`DependencyResolver` traits, `LocateError`/`GraphError`, `Workspace`, `commands::*`, `apply_version_plan`, `IdentityResolver` (§MO.4.3) |
| `callisto-moon → callisto-manifests` | normal, `default-features = false` | feature forwarding only (`cargo`/`npm`), so that §11's build line enables the right ecosystems down the graph. This crate names no `callisto-manifests` type — identity resolution routes through `callisto_graph::identity` (§MO.4.3). |
| `callisto-moon → callisto-conventional` | via `callisto-graph/inference` | not a direct edge; the adapter is `callisto-graph`'s (§G.6.4) |
| `callisto-moon → callisto-changelog` | via `callisto-graph` | not a direct edge |
| `callisto-moon → callisto-cli` | normal, **`wrapper` feature only** | the shared clap grammar and the pure renderers (§MO.2.3, §MO.7) |
| `extism-pdk` (or moon's own plugin-authoring crate) | normal | the plugin ABI. The exact PDK crate name is a moon-ecosystem fact, not a callisto design decision, and is pinned by the compatibility range above rather than respecified here. |

**A note on the `callisto-cli` edge.** §10 frames the two wrappers as siblings, not a layering
("structurally identical *as crates* … without being equally weighted *as a design
commitment*"), and everything either wrapper needs *from the core* it reaches independently.
The one exception is the argv grammar and the human renderers, which are wrapper concerns that
both wrappers must agree on exactly (§MO.2.3); those come from `callisto-cli`'s `wrapper`-featured
lib. This does not make `callisto-moon` a consumer of `callisto-cli`'s *logic* — the `wrapper`
feature compiles no handler, no `main`, and no process I/O (§CLI.8) — and it does not violate
§13 invariant 27, since argv parsing and rendering are precisely what that invariant permits
`callisto-cli` to hold.

**Deliberately absent:** `octocrab`, `reqwest`, `tokio`, or any async runtime — §9.5 and §11.
`exec_command` is a synchronous host-function call from the guest's perspective (the PDK's
`#[plugin_fn]` functions are synchronous), and `git` is the only external program this crate
ever names. Enforced by the same `xtask dep-audit` job (§G.1.7).

This crate builds for exactly one target triple in practice — `wasm32-wasip1` — since that is
the only shape moon's extism host can load (§MO.7).

### MO.2 The extension seam, concretely

Per §10, four extism plugin functions are implemented; three named categories are deliberately
not (§MO.3). Every plugin function follows the same shape: `#[plugin_fn]`,
`Json<Input> -> FnResult<Json<Output>>`, where `FnResult`'s `Err` arm is a host-visible plugin
failure — distinct from, and never used to represent, an ordinary `CommandError`/`GraphError`.
Those get rendered *into* the JSON payload and returned `Ok`, the same "errors are data, not a
crashed process" posture `callisto-cli` takes at its process boundary (§CLI.7).

> `[SPEC DECISION, not in 00-design.md: MO.2.1/MO.2.2/MO.2.3/MO.2.4's input/output types are `moon_pdk_api` (crate version 2.0.4, moon v2.4.6-compatible) re-exports, not callisto-declared shapes.]` An earlier draft of this section, written before the real `moon_pdk_api` types were verified against moon's actual wire schema, invented its own `RegisterExtensionInput`/`ExecuteExtensionInput`/`InitializeExtensionInput` shapes (a `moon_version` field on register, a flat `workspace_root` on execute, a `workspace_root` + `confirmed` pair on initialize) that never matched what moon actually sends. This revision replaces those with the real `moon_pdk_api::extension` types verified directly against the pinned crate source (`register_extension`/`define_extension_config`/`initialize_extension`'s input and output types, and `execute_extension`'s input type, are re-exports; only `ExecuteExtensionOutput` remains callisto-declared, since the real API defines no matching output shape for `execute_extension`). §MO.2.1 through §MO.2.4 below reflect the current, corrected code, not the drafted-before-verification version.

#### MO.2.1 `register_extension`

```rust
/// Declares the extension's identity to moon. Called once, before any other plugin function.
/// Both types are `moon_pdk_api::extension::{RegisterExtensionInput, RegisterExtensionOutput}`
/// re-exported as-is (§MO.2's decision note above) — not callisto-declared shapes.
#[plugin_fn]
pub fn register_extension(Json(input): Json<RegisterExtensionInput>)
    -> FnResult<Json<RegisterExtensionOutput>>;

pub struct RegisterExtensionInput {
    /// ID of the extension, as it was configured — moon's own `Id` newtype. There is no
    /// `moon_version` field on the real wire type; moon negotiates plugin/host version
    /// compatibility out of band, via the plugin manifest and `.moon/workspace.yml`'s
    /// `pluginVersion` constraint, never over this call.
    pub id: moon_common::Id,
}

pub struct RegisterExtensionOutput {
    pub name: String,
    pub description: Option<String>,
    /// This crate's own crate version — **not** `SCHEMA_VERSION` (§M.12.1). moon uses this for
    /// its own extension-version display; the JSON contract's compatibility is
    /// `schemaVersion`, carried per-report, not here.
    pub plugin_version: String,
}
```

`check_moon_version` (the `>=1.30.0, <2.0.0` compatibility check, §MO.0) is kept as a standalone, independently-tested function rather than wired into `register_extension` — the real `RegisterExtensionInput` has nothing to check a version *against*, since moon's compatibility gate lives entirely outside this wire call (see the decision note above).

#### MO.2.2 `define_extension_config`

```rust
/// Declares the schema for this extension's config section in `.moon/workspace.yml`
/// (`extensions.callisto.config`), which moon validates on the caller's behalf.
#[plugin_fn]
pub fn define_extension_config(Json(_input): Json<()>)
    -> FnResult<Json<DefineExtensionConfigOutput>>;

/// `moon_pdk_api::extension::DefineExtensionConfigOutput`, re-exported as-is.
pub struct DefineExtensionConfigOutput {
    /// `schematic::Schema`, not `serde_json::Value` — moon's own typed schema-description
    /// tree, used to render config docs/validation, not arbitrary JSON Schema. Deliberately
    /// near-empty. See the SPEC DECISION below.
    pub schema: schematic::Schema,
}
```

`callisto-moon` builds this as `schematic::Schema::structure(schematic_types::StructType::default())` — an empty `SchemaType::Struct` with no fields, `schematic`'s equivalent of "no config keys to declare here."

> `[SPEC DECISION, not in 00-design.md: `define_extension_config`'s schema declares no fields — every real callisto setting lives in `callisto.toml` (§14), validated by `callisto-graph::config` (§G.5), not by moon.]` 00-design.md names `define_extension_config` as an implemented API (§10) but does not say what it configures. §G.5.2 already establishes that `moon.yml`'s `extensions.callisto` block is per-*project* config with the highest precedence, read by the core directly as plain YAML — a second, workspace-level config surface parsed and validated by *moon itself* (which is what `extensions.callisto.config` in `.moon/workspace.yml` would be) would be a second config parser for the same settings, violating §14's single-config-system premise and reopening exactly the "two sets claiming the same thing" failure class §14 already treats as a hard error for `[[package-set]]`. Declaring the function with an empty struct schema keeps the API implemented (moon requires *some* response) without duplicating `callisto.toml`.

#### MO.2.3 `execute_extension` — subcommand dispatch

```rust
/// Every non-`completions` subcommand from §11's WASM column, dispatched to the identical
/// `callisto_graph::commands::` function `callisto-cli` calls (§G.11) — this function's entire
/// job is argv-shaped JSON in, report-shaped JSON out, matching P6's "every wrapper is a thin
/// dispatcher."
#[plugin_fn]
pub fn execute_extension(Json(input): Json<ExecuteExtensionInput>)
    -> FnResult<Json<ExecuteExtensionOutput>>;

/// `moon_pdk_api::extension::ExecuteExtensionInput`, re-exported as-is.
pub struct ExecuteExtensionInput {
    /// moon passes through everything after `moon ext callisto -- `, unparsed. Parsed with
    /// `callisto_cli::cli::Cli` — the same clap definition the binary uses (§MO.7).
    pub args: Vec<String>,
    /// moon's current context, nested rather than flat — the real wire type has no top-level
    /// `workspace_root` field. `context.workspace_root` is a `VirtualPath` (`MoonContext`,
    /// §MO.6); `.to_path_buf()` yields the (possibly WASI-virtualized) path this crate's
    /// locator/workspace loader expect. `context.working_dir` is also available but unused
    /// here — moon's resolved `workspace_root` is always preferred over any discovery this
    /// crate might otherwise attempt, since moon already did that work (§10's premise: moon is
    /// authoritative for discovery).
    pub context: MoonContext,
}

/// A callisto-declared type, **not** part of `moon_pdk_api` — the real PDK crate defines an
/// `ExecuteExtensionInput` but no matching output type for `execute_extension`; the shape of
/// what comes back is left to the extension.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ExecuteExtensionOutput {
    /// The command's own report (§M.12), serialized. Always present — `execute_extension` has
    /// no separate stdout/stderr split the way the CLI does (§13 invariant 5's
    /// stdout-only-JSON rule is a *CLI spawn-site* concern; the host boundary already gives
    /// moon a single structured channel, so the rule is satisfied trivially here, not by a
    /// redirect).
    pub report: serde_json::Value,
    /// Human-readable rendering of the same report, for moon to print to its own terminal UI
    /// when the invocation was not itself `--format json` — produced by
    /// `callisto_cli::render::human` (§CLI.5.2), never recomputed, so §13 invariant 28's
    /// attribution lines are byte-identical between the two surfaces.
    pub rendered: String,
    pub exit_code: i32,
}
```

> `[SPEC DECISION, not in 00-design.md: `callisto-moon` shares `callisto-cli`'s clap argument
> definitions **and** its human renderers, via a `wrapper`-featured `lib` target on
> `callisto-cli`, rather than re-declaring its own.]` §12.6 already requires "CLI subcommand and
> flag names" to be a fixtured contract surface, and the risk named there — a flag rename
> silently diverging between two independently-maintained parsers — is exactly what a shared
> definition forecloses structurally rather than by fixture discipline alone. The same argument
> applies to rendering: §13 invariant 28's attribution lines are a *user-visible contract*
> (§18 Q5.4 mechanism 2), and two renderers would drift. An earlier draft named this feature
> `args-only`; it covers renderers too, so `wrapper` is the accurate name (§11 records the
> rename).

Rendering rules: `--format json` invocations set `rendered` to the same 2-space-pretty JSON a
piped consumer would see (`callisto_cli::write_json` into a `String`); non-JSON invocations set
`rendered` to the human form. `report` is populated in both cases regardless — moon's own UI
decides which to show, and this crate never suppresses either.

`completions` is the one entry absent from this dispatch table (§11, §MO.8). An unrecognized
subcommand name produces a report-shaped error object rather than a panic, so a moon-side typo
or an out-of-sync `callisto-cli`/`callisto-moon` version pair fails loudly and structurally.

#### MO.2.4 `initialize_extension`

```rust
/// The host-side prompt surface for `callisto init` (§10, §11, §18 Q5.5). moon calls this when
/// the user runs the extension's initialization flow or interactively accepts callisto's
/// scaffolding offer. Unlike an earlier drafted-before-verification version of this section,
/// the payload is **not** `InitReport` (§M.12.6) — moon's real `InitializeExtensionOutput`
/// shape has no field that can carry it (see the prose below the code block).
#[plugin_fn]
pub fn initialize_extension(Json(input): Json<InitializeExtensionInput>)
    -> FnResult<Json<InitializeExtensionOutput>>;

/// `moon_pdk_api::extension::InitializeExtensionInput`, a type alias for the same
/// `InitializePluginInput` the toolchain-side `initialize_*` functions share — re-exported
/// as-is. There is no `InitReport` payload here (§M.12.6's report is not moon's wire shape)
/// and no `confirmed`/`workspace_root` pair; the only field is moon's own context.
pub struct InitializeExtensionInput {
    pub context: MoonContext,   // moon virtual paths, §MO.6
}

/// `moon_pdk_api::extension::InitializeExtensionOutput`, a type alias for
/// `InitializePluginOutput`, re-exported as-is. Its fields describe settings to inject into
/// moon's own toolchain config and prompts to ask the user — not a callisto `InitReport`.
#[serde(default)]
pub struct InitializeExtensionOutput {
    pub config_url: Option<String>,
    pub default_settings: FxHashMap<String, serde_json::Value>,
    pub docs_url: Option<String>,
    pub prompts: Vec<SettingPrompt>,
}
```

`initialize_extension` calls the identical `callisto_graph::commands::init` (§G.11) `callisto-cli` calls — there is no second `init` implementation here, only a second caller of the one that already exists (P6). Its returned `InitReport` (schema version, config path, diagnostics) is intentionally discarded rather than forced into `InitializeExtensionOutput`'s fields, since none of them can carry it faithfully; `init` still runs for its side effects (scaffolding `callisto.toml` / `.changeset`) and to propagate any error. The real wire type also carries no `confirmed`/`--yes`-equivalent flag — there is no host-side prompt surface here to relay a confirm/decline through — so `InitOptions.yes` is set unconditionally to `true`, auto-applying any reconcile drift on a re-run rather than only reporting it (the closest behavior-preserving choice to the old unconditional-write behavior; see `docs/00-design.md` §18 Q5.4 mechanism 1 for what `InitOptions.yes` gates).

### MO.3 Deliberately not implemented

Per §10, three categories are named and refused, each for a reason specific to callisto's
model, not merely "not gotten to yet":

| Function(s) | Why refused |
|---|---|
| `extend_project_graph` | Callisto **reads** moon's graph (`ProjectLocator::projects`), it never **injects** synthetic edges into it. moon's model of the workspace stays authoritative end-to-end — a callisto-computed edge (from `DepSpec` parsing) feeding back into moon's own graph would make moon's graph derived from callisto's manifest walk in one direction and callisto's cross-check derived from moon's graph in the other, a cycle that turns §G.4.6's presence-only, warn-by-default cross-check into something moon itself depends on for correctness — exactly the coupling `declared_edges()`'s non-authoritative design (§0.1, §15) exists to avoid. |
| `extend_task_command`, `extend_task_script` | Callisto does not wrap moon tasks — it has no task-execution concept at all (§9 removed the last vestige of "callisto runs things" beyond `git`). Implementing either would imply callisto participates in moon's task graph, which is a different integration shape than "moon extension that computes versions." |
| `sync_project`, `sync_workspace` | Deferred, not refused outright — §10 calls a `validate`-on-sync hook "plausible but noisy by default." A sync hook running `callisto validate` on every `moon sync` would fire far more often than a release-relevant event happens, degrading it from a useful gate to background noise a team learns to ignore (the same failure mode §6.3's `--strict` escalation is careful not to cause). Revisit if a real workflow asks for sync-triggered validation specifically, per §2.2's demand-gating posture applied to an integration point rather than an ecosystem. |

### MO.4 `MoonProjectLocator` — the `ProjectLocator` impl

#### MO.4.1 Shape

```rust
/// moon is authoritative for project *discovery* (§18 Q1a); this locator supersedes
/// `IgnoreWalkLocator` outright whenever it is constructible at all — there is no
/// "prefer moon, fall back to the walk" logic inside a single invocation, since
/// `callisto-moon` only ever runs *inside* moon (§MO.0's premise) and `callisto-cli` never
/// constructs this type at all (§CLI.4).
pub struct MoonProjectLocator<'a, R: CommandRunner> {
    runner: &'a R,
    workspace_root: PathBuf,
    identity: IdentityResolver,
    /// Cached after the first `moon project-graph --json` call — `projects()` and
    /// `declared_edges()` share one underlying host round-trip rather than two, since nothing
    /// about the workspace changes between the two calls within one invocation (P2).
    graph: OnceCell<MoonProjectGraph>,
}

impl<'a, R: CommandRunner> MoonProjectLocator<'a, R> {
    pub fn new(runner: &'a R, workspace_root: PathBuf) -> Result<Self, LocateError>;

    /// Runs `moon project-graph --json` via `self.runner` on first call, parses it into
    /// `MoonProjectGraph`, and caches it in `self.graph` (§MO.4.1) — steps 1–2 of §MO.4.2,
    /// factored out because `declared_edges()` (§MO.4.4) needs the same parsed graph
    /// `projects()` does and must not re-invoke the subprocess to get it.
    fn load_graph(&self) -> Result<&MoonProjectGraph, LocateError>;

    /// `self.identity.resolve(root, ecosystem)` (§MO.4.3), narrowed to "which ecosystem does
    /// moon's own `root` project directory resolve to" — used by `declared_edges()` (§MO.4.4),
    /// which has a bare project root path from moon's JSON and no ecosystem of its own to pass
    /// (a moon project can host both sides of a Case D pair, so there is no single
    /// `PackageId` "the" project resolves to without knowing which manifest an edge's `scope`
    /// actually concerns). Resolves by checking canonical-manifest presence at `root`
    /// (§MO.4.2 step 3) and picking the one `IdentityResolver` already assigned during
    /// `projects()` for that (path, ecosystem) pair — `Err(LocateError::OutsideWorkspaceRoot)`
    /// if `root` was never seen by `projects()` at all (a moon-graph project with neither
    /// manifest, filtered out at step 3), which is why `declared_edges()` treats this as `?`
    /// inside a `filter_map` rather than a hard error: an edge touching a manifest-less
    /// project is simply not representable as a `DeclaredEdge` and is dropped, not refused.
    fn resolve_id(&self, root: &Path) -> Result<PackageId, LocateError>;
}

/// The subset of `moon project-graph --json`'s schema callisto actually reads (§MO.4.2 step
/// 2) — not a full mirror of moon's own config types, since callisto only needs project
/// roots and their declared dependency edges, not moon's task graph, toolchain config, or
/// anything else the command emits.
#[derive(Clone, Debug, Deserialize)]
pub struct MoonProjectGraph {
    pub projects: Vec<MoonProject>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MoonProject {
    pub root: PathBuf,
    #[serde(default)]
    pub depends_on: Vec<MoonDependency>,
}

/// Mirrors moon's `ProjectDependencyConfig` (decision doc change 1) — `id`/`scope`/`source`/
/// `via` — narrowed to the three fields callisto reads. `project_root` (not moon's own `id`
/// string) is what `resolve_id` needs, since `DeclaredEdge` is `PackageId`-keyed and
/// `IdentityResolver` resolves from a path, not from moon's project-name string.
#[derive(Clone, Debug, Deserialize)]
pub struct MoonDependency {
    pub project_root: PathBuf,
    pub scope: DependencyScope,
    #[serde(default)]
    pub via: Option<String>,
}

/// moon's own dependency-scope vocabulary (decision doc change 1) — kept as a plain
/// deserialization target for `moon project-graph --json`'s JSON, immediately converted via
/// `scope_to_declared_edge_kind` (§MO.4.4) and never otherwise used, so that no *other* type
/// in this crate is built from moon's vocabulary directly (§15: "never moon's own
/// `DependencyScope`").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DependencyScope { Build, Development, Peer, Production, Root }

impl<'a, R: CommandRunner> ProjectLocator for MoonProjectLocator<'a, R> {
    fn projects(&self) -> Result<Vec<ProjectRoot>, LocateError>;
    fn declared_edges(&self) -> Option<Vec<DeclaredEdge>>;
}
```

#### MO.4.2 `projects()` — discovery

```
1. Ensure the moon project graph is loaded (§MO.4.1's cache): run
   `moon project-graph --json` via `self.runner` (§MO.5, or any CommandRunner the caller
   supplied — the locator is generic over the trait, not the concrete impl, so
   callisto-fixtures can drive it with a canned-output runner, §MO.10).
   Non-zero exit → LocateError::MoonUnavailable. moon's own CLI exiting non-zero here means
   "moon itself could not build its graph," which is a different failure than "no matches" —
   unlike `git tag --list`'s zero-matches-is-success contract (§G.9.1), moon has no
   "empty is success" case for this command.
2. Parse the JSON. A shape callisto does not recognize (a moon version whose project-graph
   schema moved) → LocateError::MoonOutputParse, not a panic and not a best-effort partial
   read — a partially-understood graph is worse than a refused one, since it could silently
   omit real packages from version consideration.
3. For each moon project, determine which ecosystems are present at its root by checking for
   Cargo.toml and/or package.json — file *presence* only, no parsing (that is
   callisto-manifests's job, invoked later during graph construction proper). A project with
   neither file is skipped, not erred: moon workspaces routinely include projects with no
   version-of-record shape at all (a docs site, a Terraform root, a CI-config-only project),
   and callisto has no opinion about those.
4. Emit one ProjectRoot { id, path, ecosystem } per detected canonical manifest — one entry
   per (path, ecosystem), never one entry carrying a list of ecosystems (§M.8). A Case D
   project therefore yields two ProjectRoots at the same path; Case D collapse into one
   Package happens downstream, in graph construction (§G.4.3), not here.
   `id` comes from `self.identity.resolve(path, ecosystem)` (§MO.4.3).
```

> `[SPEC DECISION, not in 00-design.md: ecosystem detection for a moon-discovered project is
> based on canonical-manifest *presence* (`Cargo.toml`/`package.json` existing at the
> project's root), not on moon's own `language`/`platform`/`type` project-config fields.]`
> 00-design.md never specifies how `MoonProjectLocator` assigns `ProjectRoot::ecosystem`.
> moon's `language` field is a single value per project and cannot represent Case D's
> dual-manifest shape at all — a napi package's moon project would report `language: "rust"`
> **or** `"typescript"`, never both — so trusting it would silently under-detect exactly the
> case callisto exists for (§2.1 cases E/F). File presence is the same signal
> `IgnoreWalkLocator` already uses for the non-moon path, which keeps discovery's *ecosystem
> classification* rule identical between the two locators even though *root* discovery differs:
> moon supersedes the walk for finding roots, not for classifying what is at them.

#### MO.4.3 Identity resolution

Both `projects()` (to populate `ProjectRoot::id`) and `declared_edges()` (to populate
`DeclaredEdge::from`/`to`) need a real `PackageId`, which requires reading the manifest's
declared package name — the same operation `ManifestWalkResolver::build` already performs.

> `[SPEC DECISION, not in 00-design.md: `MoonProjectLocator` obtains `PackageId` values by
> calling `callisto_graph::identity::IdentityResolver` (§G.4.2), rather than
> `callisto-moon` opening manifests itself.]` 00-design.md is silent on how
> `MoonProjectLocator` obtains `PackageId` values at all — `ProjectLocator::declared_edges()`'s
> signature (§15) takes no arguments and returns `PackageId`-bearing values, which is
> unreachable without *some* manifest read. The two options were (a) this crate depends on
> `callisto-manifests` directly and re-implements identity resolution, or (b) it calls back into
> `callisto-graph`, which already depends on `callisto-manifests` and already performs this
> exact resolution once per manifest. (b) is smaller under §13 invariant 25's own logic extended
> by analogy: "one function resolves identity" already governs tag names for exactly the `#2207`
> reason, and a second, independently-evolved identity path in `callisto-moon` is precisely the
> shape `#2207` documents, just for `PackageId` instead of `TagName`. This adds no new crate
> dependency beyond `callisto-moon → callisto-graph`, which already exists for the
> `ProjectLocator` trait and `LocateError`.

`IdentityResolver` performs the minimal read `Manifest::package_name()` needs — no dependency
parsing, no version read — and lives in a dedicated `callisto-graph::identity` module so it is
easy to audit as *not* a second cascade or graph-construction entry point (§13 invariant 27's
spirit applied one crate over). It is a struct, constructed once per invocation, rather than a
free function taking a `callisto_manifests::OpenContext`, specifically so this crate never has
to name a `callisto-manifests` type.

#### MO.4.4 `declared_edges()` — the cross-check, and the mapping table

```rust
fn declared_edges(&self) -> Option<Vec<DeclaredEdge>> {
    let graph = self.load_graph().ok()?;   // None on any failure — a cross-check that cannot
                                           // run is "no cross-check available," not a hard
                                           // error; §G.4.6 already treats `None` as
                                           // "IgnoreWalkLocator, skip the check entirely."
    Some(graph.projects.iter().flat_map(|p| {
        p.depends_on.iter().filter_map(|dep| {
            let from = self.resolve_id(&p.root).ok()?;
            let to = self.resolve_id(&dep.project_root).ok()?;
            Some(DeclaredEdge {
                from, to,
                kind: scope_to_declared_edge_kind(dep.scope),
                via: dep.via.clone(),
            })
        })
    }).collect())
}
```

```rust
/// The mapping table below, made structural. Total — every `DependencyScope` variant maps
/// to something — since `Root` (the one row with no `DepKind` equivalent) is still a real,
/// reportable `DeclaredEdgeKind`; exclusion of `Root`-scope edges from the cross-check
/// happens on the `callisto-graph` side (§G.4.6), not by this function returning a partial
/// mapping.
pub fn scope_to_declared_edge_kind(scope: DependencyScope) -> DeclaredEdgeKind;
```

**The `DependencyScope → DeclaredEdgeKind` mapping**, applied by `scope_to_declared_edge_kind`.
`DeclaredEdgeKind` is deliberately named and ordered identically to moon's own
`DependencyScope` (§15: "deliberately named and shaped after moon's own `DependencyScope`") —
so *that* mapping is the identity function, variant for variant. What is genuinely lossy, and
is the entire reason `declared_edges()` is a presence-only cross-check rather than a
kind-equality one (§G.4.6, §15), is the *second* mapping this section exists to make explicit
and undeniable: how each `DeclaredEdgeKind` relates to callisto's own `DepKind` (§M.7.1), which
is what a manifest-derived `DepEdge` actually carries.

| moon `DependencyScope` | `DeclaredEdgeKind` | Nearest `DepKind` equivalent(s) | Why the mapping is lossy |
|---|---|---|---|
| `Build` | `Build` | `DepKind::Build` | Clean 1:1 for Cargo (`[build-dependencies]`). npm has no `buildDependencies` section (§CM.5.2), so a moon `Build`-scope edge into an npm project has no manifest-derived counterpart to agree with at all — presence-only comparison tolerates this; kind-equality would flag every such edge as permanent disagreement. |
| `Development` | `Development` | `DepKind::Dev` | Clean 1:1 in name only — moon's `Development` scope is inferred from its own task/toolchain heuristics (e.g. a project referenced only by a `lint`/`test` task), not necessarily from a manifest's `devDependencies`/`[dev-dependencies]` section, so two edges can agree on *presence* while moon's provenance (`via`) and the manifest's declared kind describe different reasons for the edge existing. |
| `Peer` | `Peer` | `DepKind::Peer` | Clean 1:1 for npm's `peerDependencies`. Cargo has no peer-dependency concept (§CM.4.2), so — symmetric to the `Build` row — a moon `Peer`-scope edge into a Cargo project has no manifest counterpart; presence-only comparison, again, tolerates it. |
| `Production` | `Production` | `DepKind::Runtime` **and** `DepKind::Optional` | The one genuinely many-to-one case named explicitly in §0.1/§15: moon's `Production` scope does not distinguish an ordinary runtime dependency from an `optional = true` one (Cargo) or an `optionalDependencies` entry (npm) — both collapse into `Production` on moon's side, while callisto's model keeps them as two distinct `DepKind` variants specifically because §7.5's napi `optionalDependencies` pattern depends on the distinction. A moon `Production` edge can therefore agree in presence with either a `Runtime` or an `Optional` manifest-derived edge; kind equality is not just imprecise here, it is structurally unanswerable from moon's side alone. |
| `Root` | `Root` | *(none)* | moon's own concept of an edge to/from the workspace root project, which has no analogue in callisto's per-package `DepKind` model at all — a `Package` is never itself "the workspace root" as a graph node. `Root`-scope edges are excluded from the cross-check entirely at the `callisto-graph` end (§G.4.6), not matched against anything; `MoonProjectLocator` still reports them (the mapping is total, not partial), so the exclusion is visibly a `callisto-graph`-side filtering decision a reviewer can find, not a silent drop inside this crate. |

`source: Explicit | Implicit` (moon's other `ProjectDependencyConfig` field, decision doc
change 1) is **not** carried into `DeclaredEdge` at all — `DeclaredEdge` has no field for it
(§M.8's pinned shape), and nothing in §7 needs to distinguish an explicit `dependsOn` entry
from one moon's toolchain plugins injected implicitly; both are equally real edges for a
presence cross-check. `via` is carried through unchanged, for the human-readable diagnostic
message only (§15), never compared.

**What this table is for.** §15 requires this mapping be "explicit and documented," not merely
gestured at, precisely because a reviewer auditing why `--strict-graph` never seems to fire on a
napi workspace's `optionalDependencies` edges needs to be able to find "`Production` collapses
`Runtime`+`Optional`" written down in one place rather than reconstructing it from
`scope_to_declared_edge_kind`'s source. This table *is* that place;
`scope_to_declared_edge_kind`'s doc comment points back to it rather than restating it.

### MO.5 `MoonCommandRunner` — the `CommandRunner` impl

> `[SPEC DECISION, not in 00-design.md: the moon-side `CommandRunner::run` bridges through `warpgate_pdk`'s typed `exec`/`command_exists`/`get_host_environment`/`into_virtual_path` functions, pre-checking whether `program` exists before ever calling `exec`, rather than calling a bare `exec_command` host function directly.]` An earlier drafted-before-verification version of this section called a free `exec_command(input)` function directly and treated any of its failures uniformly. That was wrong on two counts, both found and fixed this session via black-box testing against the real wasm sandbox (`tests/moon_wasm_sandbox.rs`): first, the actual bridge is `warpgate_pdk`'s typed wrapper API, not a bare host-fn call; second, `warpgate`'s host-side `exec_command` implementation (`warpgate::host::exec_command`, in the pinned `warpgate` 0.30.5) does **not** report "command not found" as a normal `ExecCommandOutput`/error value `warpgate_pdk::exec`'s `Result` can catch — when `system_env::find_command_on_path` can't resolve `program`, the host function's own closure returns `Err(WarpgatePluginError::MissingCommand)` directly, which Extism treats as a host-function failure that aborts the *entire* plugin call (`WarpgatePluginError::FailedPluginCall`), not a value this crate's Rust code ever resumes into. That directly contradicted this crate's "`execute_extension` never panics/traps; every failure is caught into the report" invariant for the single most common real-world failure mode: the host tool being missing (`moon` itself, or `git`). The fix below checks `warpgate_pdk::command_exists` — which shells out to `which`/`Get-Command`, not to `program` itself, so a missing `program` comes back as a normal `false` rather than tripping `MissingCommand` — before ever calling `exec`, turning "program not found" into the clean `CommandError::NotFound` this method already promises, without ever calling `exec` for a program already known to be absent.

```rust
/// The moon-side `CommandRunner` (§M.10). Every call is one `warpgate_pdk::exec` host-function
/// round-trip, preceded by a `warpgate_pdk::command_exists` pre-check (see the SPEC DECISION
/// above); there is no local subprocess spawning at all, since WASI under moon's extism host
/// has no process-spawn capability of its own (§0.1 rule 2's premise: `Command` compiles under
/// wasm32-wasip1 and fails only at runtime — this impl never reaches that runtime failure
/// because it never calls `std::process::Command`, it calls the host).
pub struct MoonCommandRunner;

impl CommandRunner for MoonCommandRunner {
    fn run(&self, program: &str, args: &[&str], cwd: &Path)
        -> Result<CommandOutput, CommandError>
    {
        let host_env = warpgate_pdk::get_host_environment()
            .map_err(|e| classify_host_failure(program, &e.to_string()))?;

        if !warpgate_pdk::command_exists(&host_env, program) {
            return Err(CommandError::NotFound { program: program.to_string() });
        }

        let cwd = warpgate_pdk::into_virtual_path(cwd)   // §MO.6
            .map_err(|e| classify_host_failure(program, &e.to_string()))?;

        let input = ExecCommandInput {
            command: program.to_string(),
            args: args.iter().map(|a| a.to_string()).collect(),
            cwd: Some(cwd),
            ..ExecCommandInput::default()
        };

        match warpgate_pdk::exec(input) {
            Ok(output) => Ok(CommandOutput {
                exit_code: Some(output.exit_code),   // `warpgate_api::ExecCommandOutput.exit_code` is `i32`, never a signal-terminated `None`
                stdout: output.stdout,
                stderr: output.stderr,
            }),
            Err(e) => Err(classify_host_failure(program, &e.to_string())),
        }
    }
}
```

**§M.10's "never inherit stdio" clause is satisfied structurally, not by care taken in this impl.** A WASM guest under extism has no controlling terminal and no file-descriptor-level stdio to inherit in the first place — the `exec_command` host function `warpgate_pdk::exec` wraps is the *only* channel through which a spawned process's output can reach this code at all, and that channel is inherently captured. This is the cleaner of the two `CommandRunner` implementations with respect to that requirement precisely because the sandbox removes the failure mode entirely, rather than requiring the discipline `CliCommandRunner` has to actively enforce (§CLI.3).

**Non-zero exit is not an error**, per §M.10's contract — `output.exit_code` flows straight into `CommandOutput` regardless of value; only a failure of `command_exists`/`get_host_environment`/`into_virtual_path`/`exec` *itself* — the host could not resolve or spawn the process at all — reaches `classify_host_failure`. `CommandOutput::exit_code` is always `Some` on this surface: `warpgate_api::ExecCommandOutput.exit_code` is a plain `i32`, not an `Option<i32>`, so there is no independently observable signal-terminated state the way there can be for the CLI impl.

`classify_host_failure` and its `looks_like_not_found` helper live in `runner.rs` itself (not gated by the `pdk` feature) and are shared, unchanged, by both `CommandRunner` implementations described below — the native (non-`pdk`) impl reaches the same function from a `std::process::Command` spawn failure's message instead of a `warpgate_pdk` error's.

```rust
/// Maps a host/exec failure message to the right `CommandError` variant.
pub fn classify_host_failure(program: &str, message: &str) -> CommandError {
    if looks_like_not_found(message) {
        CommandError::NotFound { program: program.to_string() }
    } else {
        CommandError::Io { program: program.to_string(), message: message.to_string() }
    }
}

fn looks_like_not_found(msg: &str) -> bool {
    let lower = msg.to_lowercase();
    lower.contains("not found") || lower.contains("no such file")
}
```

> `[SPEC DECISION, not in 00-design.md: `classify_host_failure`'s not-found detection is a string-pattern heuristic over the host's error message, not a typed distinction.]` 00-design.md names `exec_command` as the host function this crate routes through (§10), but the extism host ABI's own error reporting for a failed spawn is a message string, not a typed error — a real ABI limitation, not a callisto design gap. The heuristic is scoped narrowly (matching the "not found" / "no such file" phrasings a host implementation is likely to produce) and degrades safely to `CommandError::Io` on a miss, which is still a correct member of `CommandRunner`'s contract (§M.10: "reserved for 'the command could not be run at all'") — just a less specific one. In practice this path is now reached rarely for the specific "program missing" case, since `command_exists` above already catches that before `exec` is ever called; `classify_host_failure` remains the catch-all for every other `command_exists`/`get_host_environment`/`into_virtual_path`/`exec` failure. `CommandError::Unsupported` is never produced by this impl, since `callisto-moon` only ever runs where the `warpgate_pdk` host functions are available by construction; that variant exists on the trait for a hypothetical WASM host without the capability, which `callisto-moon` is not.

**Version probing.** `callisto_graph::probe_git` (§G.2.4) runs once per invocation before the first real `git` call, producing `CommandError::IncompatibleVersion` identically on either surface — shared logic in the core, not re-derived here (P6).

### MO.6 The path-resolution seam

`from_virtual_path`/`to_virtual_path` are moon's own extism host functions (§10) — reached only through `warpgate_pdk`'s typed wrappers, never called raw — not a callisto-defined trait, since §15 is explicit that no separate callisto trait exists for this seam, given it is moon-side. This crate's only obligation is discipline at the boundary: **every `Path`/`PathBuf` this crate hands to core code (`Workspace::load`, `MoonCommandRunner::run`'s `cwd`, `ProjectRoot::path`) is workspace-root-relative and host-resolvable, per §M.1.3.** Two distinct conversions meet here, not one: a `VirtualPath` already received from moon (`ExecuteExtensionInput::context.workspace_root`/`InitializeExtensionInput::context.workspace_root` — nested under `MoonContext`, not a flat field, per §MO.2.3/§MO.2.4 — or a project-graph JSON payload's `root` field) is turned into the form the core's validating constructors accept via `VirtualPath::to_path_buf()`, a pure in-guest method with no host round-trip; a `Path`/`PathBuf` this crate constructs and must hand back to a host function that expects a `VirtualPath` (`MoonCommandRunner::run`'s `cwd`, §MO.5) goes through `warpgate_pdk::into_virtual_path`, a real `to_virtual_path` host-function call.

This is §M.1.3's discipline applied at one additional boundary: a path is either virtual
(moon's namespace) or workspace-relative (callisto's), never both, and this crate is the only
place the two namespaces meet — so every conversion is one-directional and explicit, never
inferred from which function happens to be holding the value.

### MO.7 `wasm32-wasip1` build and feature flags

```toml
# callisto-moon/Cargo.toml (representative)
[package]
name = "callisto-moon"
publish = false   # never published to crates.io independently — ships as a compiled .wasm
                  # artifact attached to GitHub releases (§17 v0.4), matching §11's
                  # `component: wasm | binary | both` framing for the Action's install step

[features]
default = []
# Named in §11's build line; gates nothing crate-local, since the whole crate only ever targets
# wasm32-wasip1 — carried here so the one build line's --features list reads uniformly across
# the crates it touches.
wasm = []
cargo = ["callisto-graph/cargo", "callisto-manifests/cargo"]
npm = ["callisto-graph/npm", "callisto-manifests/npm"]
inference = ["callisto-graph/inference"]

[dependencies]
callisto-model = { path = "../callisto-model" }
callisto-graph = { path = "../callisto-graph" }
callisto-manifests = { path = "../callisto-manifests", default-features = false }
callisto-cli = { path = "../callisto-cli", default-features = false, features = ["wrapper"] }
# extism-pdk (or moon's own plugin SDK crate) — pinned to the compatibility range's matching
# SDK version, §MO.0
```

Build line, reproduced from §11 for this crate specifically:

```bash
cargo build --release --target wasm32-wasip1 --no-default-features \
  --features "wasm,cargo,npm" -p callisto-moon
```

> `[SPEC DECISION, not in 00-design.md: the `callisto-moon → callisto-cli` dependency is
> feature-gated to a `wrapper` subset that compiles only the clap argument definitions and the
> pure renderers — no `main`, no `commands/` handlers, no `CliCommandRunner`, no
> `completions`.]` Without this split, depending on `callisto-cli` at all would risk pulling its
> binary-target code into the WASM build, which is both wasteful and a structural risk to §0.1
> rule 1's spirit if `callisto-cli` ever grew a moon-incompatible dependency for a CLI-only
> concern (`completions` needs shell-completion-generation machinery with no WASM story).
> Gating the shared surface behind a feature keeps `callisto-cli`'s binary-only code
> unreachable from this crate's dependency graph, satisfying the same CI-enforceable spirit as
> rule 1 even though the literal rule is phrased about the *coordination core*, not about
> `callisto-moon`.

### MO.8 Command surface

Reproduced from §11, with the dispatch target each row calls (§G.11):

| Command | `execute_extension` arm | Notes |
|---|:--:|---|
| `add` | ✓ | Builds a `Changeset`, writes the file (§CLI.6.1's flow, shared code path via the `wrapper` feature's argv types plus this crate's own file write). |
| `status` | ✓ → `commands::status` | |
| `version` | ✓ → `commands::plan_version` + `apply_version_plan` | |
| `pre enter` / `pre exit` | ✓ | Thin wrappers over `pre.json` read/write (§8, §CLI.6.4); not a named `commands::` function in §G.11's list since it is small enough to live in the dispatch layer on both wrappers identically. |
| `validate` | ✓ → `commands::validate` | v0.2. |
| `snapshot` | ✓ → `commands::plan_snapshot` + `apply_version_plan` | With `ApplyOptions { transient: true, .. }` (§G.10.2, §CLI.6.6) — identically to the CLI, and for the same non-negotiable reason: without it the run would delete every pending changeset. |
| `init` | ✓, plus `initialize_extension` (§MO.2.4) | Two entry points to the same compute function — `execute_extension`'s `init` arm for a direct `moon ext callisto -- init`, `initialize_extension` for moon's own scaffolding-prompt flow. Neither recomputes what the other does. |
| `plan-publish` | ✓ → `commands::plan_publish` | v0.2. Read-only; no shell-out beyond `git` (§11's "why WASM works here" point). |
| `compose-pr-body` | ✓ → `commands::compose_pr_body` | v0.2. |
| `tag` | ✓ → `commands::create_tags` | v0.2. Creates local tags only (§9.1, §13 inv. 24); pushing is never this crate's job either. |
| `completions` | *(absent)* | CLI-only — needs shell-integration machinery with no WASM host equivalent, and no reason to exist inside moon's own CLI, which has its own completion story. |

### MO.9 Errors

This crate introduces **no new error enum**. Every failure surfaces as one of:

- `LocateError` (`callisto-graph`) — `MoonUnavailable`, `MoonOutputParse`, and
  `IncompatibleMoonVersion` are the three variants this crate is the sole producer of (§M.13.3
  pins their text; §MO.4.2 and §MO.0 are where they are raised).
- `CommandError` (`callisto-model`) — via `MoonCommandRunner` (§MO.5).
- `GraphError`/`ManifestError`/`ConfigError` — propagated unchanged from the `commands::`
  functions this crate calls, identically to how `callisto-cli` propagates them; rendering the
  same error into `ExecuteExtensionOutput.rendered` is this crate's only local responsibility,
  matching P6.

A dedicated `callisto-moon`-local error type was considered and rejected: every failure this
crate can produce already has a home in an existing enum, and adding a wrapper would either
duplicate variants (violating the "one concept, one path" reasoning §13 invariant 25 and
§MO.4.3 both lean on) or require a `From` impl per existing enum that adds indirection without
adding information.

### MO.10 Fixture obligations

`callisto-fixtures` must carry the corpus, and this crate's `tests/` must exercise it (§CF.2's
cycle rule), for:

1. **`DependencyScope → DeclaredEdgeKind` corpus.** One fixture row per table entry in §MO.4.4,
   including the `Production`-collapses-`Runtime`+`Optional` case against both a plain runtime
   dependency and a napi `optionalDependencies` entry, asserting the cross-check in
   `callisto-graph` treats both as "agrees" (presence-only) even though a naive kind-equality
   comparison would flag one.
2. **`Root`-scope exclusion.** A moon project graph containing a `Root`-scope edge, asserting
   `MoonProjectLocator` still reports it (the mapping is total) and `callisto-graph`'s
   cross-check silently excludes it (§G.4.6) rather than either crate dropping it invisibly.
3. **Ecosystem detection from manifest presence, not `language`.** A synthetic moon
   project-graph payload where a napi project's `language` field is `"rust"` but both
   `Cargo.toml` and `package.json` exist at its root, asserting two `ProjectRoot`s are still
   emitted (§MO.4.2 steps 3–4) — the fixture that would fail if a future edit started trusting
   `language`.
4. **Identity resolution parity.** The same workspace fixture run once through
   `IgnoreWalkLocator` and once through `MoonProjectLocator` (with a canned project-graph
   payload describing the identical workspace), asserting every resulting `PackageId` is
   byte-identical between the two — the concrete test of §MO.4.3's one-resolution-path claim.
5. **`MoonCommandRunner` replay corpus.** A canned `exec_command` failure-message corpus
   (`"exec: \"nonexistent\": executable file not found in $PATH"`, a generic host I/O error
   string, a well-formed non-zero exit) asserting `classify_host_failure` lands `NotFound` vs.
   `Io` correctly on the recognizable cases and degrades to `Io` — never a panic, never a wrong
   `NotFound` — on an unrecognized one.
6. **moon-version compatibility gate.** One fixture below the pinned floor, one at it, one above
   the ceiling, one malformed (a version string that does not parse at all) — each asserting
   `LocateError::IncompatibleMoonVersion`/`MoonOutputParse` fires at `register_extension` time,
   not deferred to first command use.
7. **Argv-grammar and render parity with `callisto-cli`.** Since this crate consumes
   `callisto-cli`'s `wrapper`-featured types (§MO.2.3), this is largely free — but a fixture that
   constructs the same `ExecuteExtensionInput.args` and CLI `argv` from one shared table and
   asserts identical parsed results *and* identical `rendered` output is still worth carrying
   explicitly, since it is the fixture that would fail first if the feature gating in §MO.7 ever
   accidentally diverged the two.
8. **`wasm32-wasip1` fixture run**, matching §13 invariant 26's core-crate requirement but
   extended to this crate specifically, since it is the one crate that *only* ever runs on that
   target: its suite must pass under `wasmtime` with only the workspace root preopened and a
   stubbed `exec_command` host import (a fixture-provided WASI host mock, not a live moon
   runtime), which is what makes fixtures 1–7 runnable in CI at all without a moon binary.

### MO.11 Index of `[SPEC DECISION]` flags

| # | Section | Decision |
|---|---|---|
| 1 | §MO.0 | Pinned moon compatibility range `>=1.30.0, <2.0.0`, checked once per invocation in `register_extension`. |
| 2 | §MO.2.2 | `define_extension_config` declares a near-empty schema — `callisto.toml` stays the single config surface. |
| 3 | §MO.2.3 | `callisto-moon` shares `callisto-cli`'s clap definitions **and** renderers via a `wrapper`-featured `lib` target. |
| 4 | §MO.4.2 | Ecosystem detection for a moon-discovered project uses manifest-file presence, not moon's `language`/`platform` fields. |
| 5 | §MO.4.3 | `PackageId` resolution calls `callisto_graph::identity::IdentityResolver` rather than `callisto-moon` naming a `callisto-manifests` type. |
| 6 | §MO.5 | `classify_host_failure`'s not-found detection is a string-pattern heuristic, degrading safely to `CommandError::Io`. |
| 7 | §MO.7 | The `callisto-moon → callisto-cli` edge is feature-gated to `wrapper`, keeping binary-only code unreachable from the WASM build. |

### MO.12 Deliberately not owned by this crate

| Concept | Owner | Why not here |
|---|---|---|
| Release semantics — aggregation, cascade, groups, mutation ordering | `callisto-graph` | P6, §13 inv. 27's spirit extended to this crate: `callisto-moon/src` contains no graph-construction or cascade code, only dispatch. |
| `Manifest` read/write | `callisto-manifests` | Consumed through the trait during the `commands::` calls this crate dispatches to; never opened directly here, and never named as a type (§MO.4.3). |
| Argv grammar's *definition*, human rendering | `callisto-cli` | §MO.2.3 — this crate consumes both, owns neither. |
| `DeclaredEdge`/`DeclaredEdgeKind`/`ProjectRoot`/`PackageId`/`CommandRunner`/`CommandOutput`/`CommandError`/plan-report types | `callisto-model` | §15/§M — this crate constructs and consumes these values, never redefines them. |
| `ProjectLocator`/`DependencyResolver` trait definitions, `LocateError`, `GraphError`, `IdentityResolver` | `callisto-graph` | §15 — `MoonProjectLocator` implements the trait; it does not own it. |
| Publishing anything | the calling workflow | §9 — unchanged by which wrapper is in use; `callisto-moon` produces the identical read-only `plan-publish` output `callisto-cli` does. |
| moon's own extension-loading, host-function ABI versioning, `.moon/workspace.yml` schema beyond `extensions.callisto` | moon itself | Outside callisto's scope entirely; this crate targets a pinned compatibility range (§MO.0) rather than tracking moon's internals. |

---

## 10. `callisto-fixtures`

**Purpose.** The shared corpus and test infrastructure: byte-compat fixture files, plan/report
golden files, the in-memory `DependencyResolver` that justifies that trait existing, and an
in-memory `CommandRunner` double.

**License:** AGPL-3.0 (§16's "everything else"). **Published:** no — `publish = false`,
dev-only. **Milestone:** v0.1, alongside the crates it tests (§17).

> **Reconciliation note.** An earlier draft of this crate's spec proposed MIT/Apache-2.0 and a
> `callisto-model`-only dependency. Both are corrected here. §16 names exactly two permissive
> crates (`callisto-format`, `callisto-model`) and puts everything else under AGPL, and a
> dev-only unpublished crate that links AGPL crates to test them cannot meaningfully be
> permissive anyway. And a `callisto-model`-only dependency contradicts the crate's own stated
> purpose: decision doc change 3 makes it the home of the in-memory `DependencyResolver`
> implementor, and that trait is `callisto-graph`'s. §CF.2 gives the actual edge set. §11
> records both corrections.

### CF.1 What this crate is for

Two things, and §15 names both:

1. **The fixture corpus** — "byte-compat corpus, plan-schema golden files … shared by
   `callisto-cli`'s and `callisto-moon`'s test suites." P7's "fixtured contract" made literal.
2. **The in-memory `DependencyResolver` impl** — decision doc change 3 is explicit that this is
   the *reason `DependencyResolver` is a trait at all* rather than a concrete
   `ManifestWalkResolver` struct, and that if this suite ended up not needing one, the trait
   should be demoted. It is therefore load-bearing, not a convenience.

It contains **no coordination logic of its own**. Every algorithm it exercises lives in the
crate that owns it; this crate supplies inputs, expected outputs, and the two test doubles
(`GraphBuilder`, `ReplayCommandRunner`) that let those algorithms run without a filesystem or a
`git` binary.

#### CF.1.1 What the corpus enforces

- **Byte-compatibility (P1, §6).** Round-trip fixtures for `.changeset/*.md` and `pre.json`,
  and the `bump_version` golden table — all vendored from `@changesets/cli`'s own corpus where
  provenance is claimable (§F.9's item 7).
- **The JSON contract (§12.5, §13 inv. 14).** One golden file per `Report::COMMAND`, at
  `schemaVersion` 1, including empty-array and optional-field-absent variants so that a field
  moving between mandatory and optional fails a fixture.
- **The wider contract surface §12.6 names.** Tag naming, CLI subcommand and flag names, and
  manifest write formatting — "not JSON shape alone."
- **Cross-wrapper agreement.** `callisto-cli` and `callisto-moon` run the same corpus, so
  "same workspace state ⇒ same JSON" is fixtured rather than assumed (§MO.10 item 7).

**Comparison semantics for JSON goldens** follow §M.12.1: golden files are 2-space
pretty-printed with a trailing newline for readable diffs, but comparison is on *parsed*
`serde_json::Value`s, so whitespace and key order are not part of the contract and a
`serde_json` formatting change cannot break the suite. Byte-identity comparison is used only
where byte-identity *is* the claim: the changeset and `pre.json` round-trips (P1) and the
manifest round-trip-fidelity corpus (§12.6).

### CF.2 Dependencies, features, and the cycle rule

Every other crate dev-depends on `callisto-fixtures`, and `callisto-fixtures` depends on the
crates whose types its builders construct. Cargo permits that shape — a dev-dependency edge may
close a cycle that a normal-dependency edge may not — but only if the *normal* edges stay
acyclic, which constrains what this crate may depend on and which feature each consumer enables.

> `[SPEC DECISION, not in 00-design.md: `callisto-fixtures` splits its dependencies behind a
> `graph` feature — default builds depend on `callisto-model` only; `graph` adds
> `callisto-format`, `callisto-manifests`, `callisto-conventional`, `callisto-changelog`, and
> `callisto-graph`.]` §15 describes this crate as holding a corpus shared by both wrappers'
> test suites *and* the in-memory `DependencyResolver`, which needs `callisto-graph`. A single
> undifferentiated dependency set would put `callisto-format` in a normal-dependency cycle with
> itself (`callisto-format` dev-depends on the corpus; the corpus would normal-depend on
> `callisto-format`). Splitting by feature lets `callisto-format` enable default features only
> — the corpus data it needs is plain `&'static str` tables and file contents, requiring no
> `callisto-format` type — while `callisto-graph`, `callisto-manifests`,
> `callisto-conventional`, `callisto-changelog`, `callisto-cli`, and `callisto-moon` all
> enable `graph`. The wrappers need it for `ReplayCommandRunner` specifically (§CF.3.4), and
> nothing about them closes a normal-dependency cycle, since `callisto-fixtures` depends on
> neither.

```toml
# callisto-fixtures/Cargo.toml (representative)
[package]
name = "callisto-fixtures"
publish = false
license = "AGPL-3.0"

[features]
default = []
# The builders and scenario runner. Everything a test needs that is not just corpus bytes.
graph = [
  "dep:callisto-format", "dep:callisto-manifests", "dep:callisto-conventional",
  "dep:callisto-changelog", "dep:callisto-graph",
]
# Random-input generation for the property tests in §CF.5. Optional because they are slow.
proptest = ["dep:proptest"]

[dependencies]
callisto-model = { path = "../callisto-model" }
callisto-format = { path = "../callisto-format", optional = true }
callisto-manifests = { path = "../callisto-manifests", optional = true }
callisto-conventional = { path = "../callisto-conventional", optional = true }
callisto-changelog = { path = "../callisto-changelog", optional = true }
callisto-graph = { path = "../callisto-graph", optional = true }
serde_json = "1"
proptest = { version = "1", optional = true }
```

**Who enables what:**

| Consumer | Feature it enables | Why |
|---|---|---|
| `callisto-format` | default (none) | corpus bytes and the `bump_version` table only — avoids the normal-dependency cycle |
| `callisto-model` | default (none) | §M.17's corpus is model-typed or plain text |
| `callisto-manifests`, `callisto-conventional`, `callisto-changelog`, `callisto-graph` | `graph` | builders, scenarios, `ReplayCommandRunner` |
| `callisto-cli`, `callisto-moon` | `graph` | corpus + golden files **and** `ReplayCommandRunner` — §CLI.9 item 2 replaces `CliCommandRunner` with a canned-output double to prove stdout purity, and §MO.10 items 4–5 drive `MoonProjectLocator` and `classify_host_failure` from canned `moon`/`exec_command` output; all three are `#[cfg(feature = "graph")]` types (§CF.3.4). An earlier draft of this table said "default (none)", which would have left both wrappers' most load-bearing fixtures unwritable. |

Only `callisto-format` and `callisto-model` enable default features, and only
`callisto-format` is required to: it is the one crate that would otherwise sit in a *normal*
dependency cycle with the corpus. The wrappers are dev-only consumers of a crate that does not
depend on them, so `graph` closes no cycle there — `callisto-fixtures` normal-depends on
`callisto-graph`, which `callisto-cli` and `callisto-moon` also normal-depend on, and neither
wrapper is reachable from `callisto-fixtures` at all (§CF.7). Note the interaction with
§CLI.9 item 5: enabling `graph` puts `callisto-manifests` in `callisto-cli`'s **dev**-edge
closure, which is exactly why that audit checks for a *direct* dependency rather than a
transitive one.

**Where per-crate test suites live.** The corpus and the builders live here; the *assertions*
live in each crate's own `tests/` directory, dev-depending on this crate. That is what keeps
`callisto-cli`'s and `callisto-moon`'s obligations (§CLI.9, §MO.10) discharge-able without this
crate depending on either wrapper — which it must not, since both dev-depend on it.

### CF.3 Types

#### CF.3.1 `GraphBuilder` — the in-memory `DependencyResolver`

```rust
/// Constructs a `DependencyResolver` from literal `Package`/`DepEdge` values, with no
/// filesystem, no `Manifest` calls, and no `ProjectLocator`. The named second implementor
/// decision doc change 3 cites as the trait's justification.
///
/// Used to unit-test cascade (§G.7), group alignment (§G.8), aggregation (§G.6), and
/// `toposort` (§G.3.2) against graphs where every node and edge is under test control —
/// isolated from graph-construction ambiguities and from I/O entirely.
#[cfg(feature = "graph")]
pub struct GraphBuilder { /* packages, edges */ }

impl GraphBuilder {
    pub fn new() -> Self;

    /// Adds a package. Prefer explicit `PackageId::Bare("name")` /
    /// `PackageId::Prefixed { .. }` construction over inference, so fixture graphs are
    /// deterministic by construction rather than by resolution order.
    pub fn package(self, id: impl Into<PackageId>,
                   f: impl FnOnce(PackageBuilder) -> PackageBuilder) -> Self;

    /// Adds a dependency edge. Both endpoints must already have been added.
    /// `from_manifest` (§M.7.3) defaults to the `from` package's first canonical manifest and
    /// `inherited` to `false`.
    pub fn edge(self, from: impl Into<PackageId>, to: impl Into<PackageId>,
                kind: DepKind, spec: DepSpec) -> Self;
    pub fn edge_from_manifest(self, from: impl Into<PackageId>, to: impl Into<PackageId>,
                              kind: DepKind, spec: DepSpec, manifest: PathBuf) -> Self;
    /// §G.4.4's Cargo-inheritance case: `from_manifest` is the workspace root and
    /// `inherited` is `true`, so the edge produces a
    /// `DepWriteTarget::CargoWorkspaceDependency` rewrite (§G.7.3). `kind` stays the
    /// *member's* section kind, which is what §G.15 item 6's dev-inheritance row asserts.
    pub fn edge_inherited(self, from: impl Into<PackageId>, to: impl Into<PackageId>,
                          kind: DepKind, spec: DepSpec, root_manifest: PathBuf) -> Self;

    /// Consumes the builder. Fails on the same structural errors real graph construction does
    /// — `DuplicatePackage`, `UnknownPackage` for a dangling edge endpoint — so a malformed
    /// fixture fails as a fixture error rather than producing a silently wrong graph.
    pub fn build(self) -> Result<InMemoryGraph, GraphError>;
}

/// Field-for-field builder for `Package` (§M.6.1). Defaults: `release_trigger = Changeset`,
/// `publish_to = []`, `changelog = None`, `tag_template = None`, one `Canonical`
/// `ManifestDecl` at `<id>/Cargo.toml` or `<id>/package.json` per the chosen ecosystem.
pub struct PackageBuilder { /* … */ }

impl PackageBuilder {
    pub fn release_trigger(self, rt: ReleaseTrigger) -> Self;
    pub fn publish_to(self, targets: Vec<PublishTarget>) -> Self;
    pub fn changelog(self, path: Option<PathBuf>) -> Self;
    pub fn tag_template(self, t: Option<TagTemplate>) -> Self;
    pub fn manifest(self, decl: ManifestDecl) -> Self;   // repeatable; Case D and napi shapes
    pub fn version(self, v: Version) -> Self;            // the "on-disk" base for `base` maps
}

pub struct InMemoryGraph { /* … */ }

impl DependencyResolver for InMemoryGraph {
    fn packages(&self) -> impl Iterator<Item = &Package>;
    fn dependencies_of(&self, id: &PackageId) -> impl Iterator<Item = &DepEdge>;
    fn dependents_of(&self, id: &PackageId) -> impl Iterator<Item = &DepEdge>;
    fn toposort(&self, subset: &HashSet<PackageId>) -> Result<Vec<PackageId>, GraphError>;
}
```

**What it deliberately cannot do**, so that a test reaching for it knows when to use a real
workspace fixture instead: it reads no manifests (every package's state is what the builder was
given), it calls no `CommandRunner` (tag resolution and commit inference are tested against
`ReplayCommandRunner` in the crates that own them), and it implements no `ProjectLocator`
(discovery is tested in `callisto-graph`'s own suite against real fixture directories).

#### CF.3.2 `Scenario` — parameterized cascade/aggregation tests

```rust
/// A bundled graph, severity seed, and expected outcome. Exists purely to reduce boilerplate
/// in table tests; it is not part of any contract.
#[cfg(feature = "graph")]
pub struct Scenario {
    pub name: &'static str,
    pub graph: InMemoryGraph,
    pub base: BTreeMap<PackageId, Version>,
    pub groups: GroupTable,
    pub cascade: CascadeConfig,
    pub seed: BTreeMap<PackageId, Severity>,
    pub expected_severities: BTreeMap<PackageId, Severity>,
    pub expected_reasons: BTreeMap<PackageId, BumpReason>,
    pub expected_rewrites: Vec<SpecRewrite>,
    pub expected_diagnostics: Vec<DiagnosticCode>,
    pub expect_error: Option<&'static str>,   // GraphError variant name
}

impl Scenario {
    pub fn builder(name: &'static str) -> ScenarioBuilder;
    /// Runs `callisto_graph::cascade::run_cascade` and asserts every `expected_*` field.
    pub fn assert(&self);
}
```

#### CF.3.3 Corpus access

```rust
/// Corpus files are embedded with `include_str!`/`include_bytes!` rather than read from disk
/// at test time, so the `wasm32-wasip1` fixture run (§13 inv. 26) needs nothing preopened
/// beyond the workspace root and a test cannot fail because of a working-directory assumption.
pub mod corpus {
    /// `(filename, contents)` for every fixture under `data/changesets/valid/`.
    pub fn changesets_valid() -> &'static [(&'static str, &'static str)];
    /// `(filename, contents, expected_ParseError_variant_name)`.
    pub fn changesets_invalid() -> &'static [(&'static str, &'static str, &'static str)];
    /// `(current, severity, expected)` triples (§F.9.3).
    pub fn bump_version_table() -> &'static [(&'static str, &'static str, &'static str)];
    /// `(base, severity, tag, current, expected)` rows (§F.9.4, §F.6.3).
    pub fn bump_prerelease_table()
        -> &'static [(&'static str, &'static str, &'static str, &'static str, &'static str)];
    pub fn pre_json() -> &'static [(&'static str, &'static str)];
    /// One entry per `Report::COMMAND`, plus the named variants (`plan-publish-empty`, …).
    pub fn report_goldens() -> &'static [(&'static str, &'static str)];
    pub fn package_ids() -> &'static [(&'static str, Option<PackageId>)];
    // … one accessor per corpus directory in §CF.4.
}
```

#### CF.3.4 `ReplayCommandRunner`

```rust
/// An in-memory `CommandRunner` (§M.10) that replays canned output for a table of
/// `(program, args)` keys. The reason `callisto-graph`'s and `callisto-conventional`'s fixture
/// suites need no `git` binary and no process execution under WASI at all — which is the
/// point of the seam existing, and this double is what proves the seam is real (§G.15 item 11).
pub struct ReplayCommandRunner { /* … */ }

impl ReplayCommandRunner {
    pub fn new() -> Self;
    /// Registers a response. An unregistered `(program, args)` pair is a panic with the exact
    /// invocation printed, not a silent empty success — a test that shells out somewhere
    /// unexpected should say so loudly.
    pub fn on(self, program: &str, args: &[&str], out: CommandOutput) -> Self;
    /// Registers a `CommandError` instead of an output, for §MO.10 item 5's classification
    /// corpus and for `CommandError::NotFound` paths.
    pub fn on_error(self, program: &str, args: &[&str], err: CommandError) -> Self;
    /// Every invocation seen, in order — lets a test assert *that* `git update-ref` ran
    /// (§C.6) as well as what it returned.
    pub fn calls(&self) -> &[(String, Vec<String>)];
}

impl CommandRunner for ReplayCommandRunner { /* … */ }
```

### CF.4 Corpus layout

```
callisto-fixtures/
├── src/
│   ├── lib.rs             # corpus accessors, auto-trait compile assertions (§M.17 item 7)
│   ├── corpus.rs          # include_str!/include_bytes! tables
│   ├── graph_builder.rs   # feature = "graph" — GraphBuilder, PackageBuilder, InMemoryGraph
│   ├── scenario.rs        # feature = "graph" — Scenario, ScenarioBuilder
│   └── runner.rs          # feature = "graph" — ReplayCommandRunner
└── data/
    ├── changesets/
    │   ├── valid/*.md               # §F.9; SOURCE.md records upstream provenance
    │   └── invalid/*.md + cases.toml
    ├── pre/*.json                    # §F.9
    ├── bump-version/                 # §F.9
    │   ├── table.json                #   §F.9.3 — bump_version
    │   └── prerelease.json           #   §F.9.4 — bump_prerelease (§F.6.3)
    ├── package-ids.txt               # §M.17 item 1
    ├── tag-templates/                # §M.17 items 2–3
    │   ├── <case>/{template.txt,glob.txt,tags.txt,expected.txt}
    │   └── errors/cases.toml
    ├── severity-combinations.txt     # §M.17 item 4
    ├── reports/                      # §M.17 item 6 — one per Report::COMMAND, plus variants
    │   ├── plan-publish.json, plan-publish-empty.json,
    │   │   plan-publish-custom-registry.json
    │   ├── version.json, version-with-reasons.json
    │   ├── status.json, snapshot.json, compose-pr-body.json,
    │   │   compose-pr-body-overflow.json
    │   └── validate-ok.json, validate-error.json, tag.json, init.json
    ├── manifests/                    # §CM.9
    │   ├── roundtrip/{cargo,npm}/    #   unusual formatting: tabs, 4-space, CRLF, no trailing
    │   │                             #   newline, comments around a dependency table
    │   ├── depspecs/{cargo,npm}.toml #   one row per §CM.4.2/§CM.5.2 table entry
    │   ├── cargo-workspace-inherit/  #   §CM.9 item 4
    │   ├── napi-optional-deps/       #   §CM.9 item 5
    │   └── lockfiles/                #   §CM.9 item 6
    ├── commits/                      # §C.9 — header, footer, severity, pre-major corpora
    ├── changelogs/                   # §CL.9 — before/after prepend pairs
    ├── moon/                         # §MO.10 — canned `moon project-graph --json` payloads
    │                                 #   and `exec_command` failure-message strings
    └── dep-audit/                    # §G.15 item 12 — a manifest that MUST fail the audit
```

### CF.5 Property tests (`proptest` feature)

Optional, off by default because they are slow, and deliberately narrow — they cover the two
places where an exhaustive table is infeasible and an invariant is cheap to state:

1. **Cascade convergence.** For a random DAG of ≤ 20 packages and a random severity seed,
   `run_cascade` terminates within `convergence_bound(n)` (§G.7.6) and its result is independent
   of worklist pop order (severities are a max-fold, so the fixed point is unique — §G.7.6's
   claim, checked rather than asserted).
2. **Severity aggregation.** `max` over a random multiset of `Severity` values equals the
   fold-based aggregation, for every subset — the machine-checked form of §M.17 item 4.

Everything else is a table, not a property: the cascade *table* (§G.15 item 1) is finite and
exhaustively enumerable, and enumerating it is more informative than sampling it.

### CF.6 Obligations index

Where each crate's fixture obligations are discharged. The corpus lives here; the assertion
lives where the "asserted in" column says.

| Obligation | From | Corpus | Asserted in |
|---|---|---|---|
| `PackageId::parse` | §M.17.1 | `data/package-ids.txt` | `callisto-model/tests/` |
| Tag round-trip | §M.17.2 | `data/tag-templates/` | `callisto-model/tests/` |
| Tag template rejections | §M.17.3 | `data/tag-templates/errors/` | `callisto-model/tests/` |
| Severity aggregation | §M.17.4 | `data/severity-combinations.txt` | `callisto-model/tests/` |
| `Version` equality/comparison | §M.17.5 | — (literal pairs) | `callisto-model/tests/` |
| Report golden files | §M.17.6 | `data/reports/` | `callisto-model/tests/` |
| Auto-trait assertions | §M.17.7 | — (compile test) | `callisto-fixtures/src/lib.rs` |
| Changeset round-trip + negative variants | §F.9.1–2 | `data/changesets/` | `callisto-format/tests/` |
| `bump_version` golden table | §F.9.3 | `data/bump-version/` | `callisto-format/tests/` |
| `bump_prerelease` golden table | §F.9.4 | `data/bump-version/prerelease.json` | `callisto-format/tests/` |
| `pre.json` round-trip | §F.9.5 | `data/pre/` | `callisto-format/tests/` |
| CRLF normalisation | §F.9.6 | `data/changesets/valid/` (CRLF twin) | `callisto-format/tests/` |
| Manifest round-trip fidelity | §CM.9.1 | `data/manifests/roundtrip/` | `callisto-manifests/tests/` |
| `DepSpec` parse corpus | §CM.9.2 | `data/manifests/depspecs/` | `callisto-manifests/tests/` |
| Round-trip rewrite corpus | §CM.9.3 | `data/manifests/depspecs/` | `callisto-manifests/tests/` |
| Cargo workspace inheritance | §CM.9.4 | `data/manifests/cargo-workspace-inherit/` | `callisto-manifests/tests/` |
| `optionalDependencies` rewrite | §CM.9.5 | `data/manifests/napi-optional-deps/` | `callisto-manifests/tests/` |
| Lockfile-role refusal | §CM.9.6 | `data/manifests/lockfiles/` | `callisto-manifests/tests/` |
| Commit header/footer/severity/pre-major | §C.9.1–4 | `data/commits/` | `callisto-conventional/tests/` |
| Pre-cursor ref round-trip | §C.9.5 | — (`ReplayCommandRunner`) | `callisto-conventional/tests/` |
| Changelog prepend goldens | §CL.9.1–4 | `data/changelogs/` | `callisto-changelog/tests/` |
| `extract_section` round-trip | §CL.9.5 | `data/changelogs/` | `callisto-changelog/tests/` |
| Cascade table, exhaustive (incl. `mode = always` non-inertness) | §G.15.1 | — (`Scenario` table) | `callisto-graph/tests/` |
| `coverage` per `DepSpec`, incl. the `CargoBare` caret table | §G.15.1b | — (literal pairs) | `callisto-graph/tests/` |
| Fixpoint convergence + attribution | §G.15.2 | — (`GraphBuilder`) | `callisto-graph/tests/` |
| Peer escalation into a fixed group | §G.15.3 | — (`GraphBuilder`) | `callisto-graph/tests/` |
| Linked joint vs. cascade | §G.15.4 | — (`GraphBuilder`) | `callisto-graph/tests/` |
| Rewrite round-trip policy | §G.15.5 | `data/manifests/depspecs/` | `callisto-graph/tests/` |
| Cargo inheritance de-dup + kind preservation | §G.15.6 | `data/manifests/cargo-workspace-inherit/` | `callisto-graph/tests/` |
| Typed version write targets, workspace-version conflict | §G.15.6b | `data/manifests/cargo-workspace-inherit/` | `callisto-graph/tests/` |
| Group validation | §G.15.7 | — (`GraphBuilder` + config strings) | `callisto-graph/tests/` |
| `toposort` | §G.15.8 | — (`GraphBuilder`) | `callisto-graph/tests/` |
| `declared_edges` cross-check | §G.15.9 | `data/moon/` | `callisto-graph/tests/` |
| Empty-changeset validation (config key **and** flag) | §G.15.10 | `data/changesets/` + `ReplayCommandRunner` | `callisto-graph/tests/` |
| New-member force-set to the group target | §G.15.10b | — (`GraphBuilder`) | `callisto-graph/tests/` |
| Snapshot is non-destructive (`ApplyOptions::transient`) | §G.15.10c | fixture workspace + `data/changesets/` | `callisto-graph/tests/` |
| Pre-major boundary corpus | §G.15.10d | — (literal severities/versions) | `callisto-graph/tests/` |
| `xtask dep-audit` self-test | §G.15.12 | `data/dep-audit/` | `xtask/tests/` |
| CLI subcommand/flag snapshot | §CLI.9.1 | — (clap introspection) | `callisto-cli/tests/` |
| `--format json` stdout purity | §CLI.9.2 | `data/reports/` + `ReplayCommandRunner` | `callisto-cli/tests/` |
| Attribution-line rendering | §CLI.9.3 | — | `callisto-cli/tests/` |
| Exit-code corpus | §CLI.9.4 | — | `callisto-cli/tests/` |
| `callisto-cli` dependency audit | §CLI.9.5 | — (`cargo metadata`) | CI job |
| `add` round-trip | §CLI.9.6 | — | `callisto-cli/tests/` |
| moon scope mapping, `Root` exclusion, ecosystem detection, identity parity, host-failure classification, version gate, argv/render parity | §MO.10.1–7 | `data/moon/` | `callisto-moon/tests/` |
| `wasm32-wasip1` run | §M.17.8, §F.9, §CM.9.7, §C.9.6, §CL.9.6, §G.15.11, §MO.10.8 | the whole corpus | CI job, `wasmtime`, workspace root preopened only |

### CF.7 Deliberately not owned by this crate

| Concept | Owner | Why not here |
|---|---|---|
| Any algorithm under test | the crate that owns it | This crate supplies inputs, expected outputs, and doubles. A behaviour implemented here would be untested by construction. |
| Per-crate assertions | each crate's `tests/` | §CF.2's cycle rule, and so a failing test names the crate that broke. |
| Real-workspace integration tests | `callisto-cli`/`callisto-moon` `tests/` | Those need real directories and a real (or replayed) `git`; this crate's builders are for the pure-logic half. |
| CI job definitions | the workspace's CI config / `xtask` | This crate is a library; `wasmtime` invocation and `cargo metadata` auditing are job concerns. |

---

## 11. New decisions made while writing this spec

Two lists. **§11.1** collects every `[SPEC DECISION, not in 00-design.md: …]` marker, by crate,
so they can be reviewed as a batch rather than found in prose. **§11.2** collects the
reconciliations made while merging the ten independently-drafted crate specs into this
document — cases where two drafts disagreed and one shape had to win.

Neither list changes anything in `00-design.md`. Every item is either filling a gap the design
doc left, or choosing between two readings it permits. Anything that would *contradict*
`00-design.md` is a bug in this document, not a decision.

### 11.1 Spec decisions, by crate

**`callisto-model` (§M.16)** — 23 decisions. Summary of the load-bearing ones:

| # | Section | Decision |
|---|---|---|
| M1 | §M.1.3 | All model paths are workspace-root-relative and UTF-8; validating constructors reject otherwise. |
| M2 | §M.4.1 | `Version` is a grammar-tagged struct, not a newtype over `semver::Version`. |
| M3 | §M.4.2 | `Version`'s `Deserialize` parses under `SemVer`; a non-SemVer ecosystem requires a grammar discriminator plus a `schemaVersion` bump. |
| M4 | §M.4.2 | `Ord` is deliberately not implemented for `Version`; cross-grammar comparison is `Err`. |
| M5 | §M.4.5 | The crate edge is `callisto-format → callisto-model`; `callisto-model` depends on no `callisto-*` crate. `Severity` and `Version` are declared here. |
| M6 | §M.6.1 | `Package` keeps exactly §5.1's six fields; `pre-major-inference` and group membership live with resolved config. |
| M7 | §M.6.1 | napi platform packages are `ManifestRole::Platform` manifests of the main `Package`, not separate `Package` values — makes §13 inv. 20 structural. |
| M8 | §M.7.2 | `DepSpec::Catalog` is `Coverage::Unknown` and is never rewritten. |
| M9 | §M.7.3 | `DepEdge` gains `from_manifest: PathBuf`; one edge per (declaring manifest, entry). |
| M10 | §M.8 | `ProjectLocator` emits one `ProjectRoot` per (root path, ecosystem); Case D collapse stays in graph construction. |
| M11 | §M.9.1 | A template with no literal anchor is rejected at config-load (`NoLiteralAnchor`). |
| M12 | §M.9.4 | `last_tag_for` is split: pure select in `callisto-model`, `git tag --list` in `callisto-graph`. |
| M13 | §M.10 | `CommandRunner`/`CommandOutput`/`CommandError` live in `callisto-model`; the trait stays dyn-compatible. |
| M14 | §M.11.2 | One `Diagnostic` type plus an optional `diagnostics` array on every report envelope. |
| M15 | §M.12.1 | Golden files are 2-space pretty-printed; comparison is on parsed values. |
| M16 | §M.12.2 | Plan `publishTo` is a registry-key string with an optional sibling `registry` URL. |
| M17 | §M.12.3 | `BumpRecord` gains an optional structured `reason: BumpReason`. |
| M18 | §M.12.4 | `StatusReport.packages[]` shape specified (optional, length-gated). |
| M19 | §M.12.6 | Minimal shapes specified for `validate`, `tag`, and `init` JSON output. |
| M20 | §M.13.2 | `ManifestError` is declared in `callisto-model`, not `callisto-manifests`. |
| M21 | §M.7.3 | Cargo workspace inheritance is signalled by `DependencyEntry::inherited`/`DepEdge::inherited`, not by anything inside `DepSpec`. |
| M22 | §M.2 | `PackageId::name()` is not a publish-target name; `IdentityIndex::native_name` is, because Case D allows two divergent native names for one identity. |
| M23 | §M.12.2 | `ReleaseEntry.changelog_section` is optional and is read back off the written `CHANGELOG.md`. |

**`callisto-format` (§F.10)** — 9 decisions: filesystem-free API; CRLF→LF on read, LF on write;
the `needs_quoting` character set; duplicate raw names are a hard parse error; `bump_version`
returns `Result` for the grammar precondition; `IndexMap` for `initial_versions`; `pre.json`
byte-shape plus §F.6.3's pre-release arithmetic, but not §8's cross-package orchestration; and
`Versioning::bump_prerelease` as the home of the `pre.N` counter. Plus the reversed
`Severity`-ownership flag (§11.2).

**`callisto-manifests` (§CM.10)** — 8 decisions: mutating methods persist immediately;
`OpenContext`/`open()` shape; lockfile-role refusal; `WorkspaceCargoResolver`/
`WorkspaceInheritance` split; `PackageJson`'s format fingerprint and 2-space fallback;
`serde_json/preserve_order` unifies workspace-wide; `WorkspaceKind` from lockfile presence;
demand-gated formats get refusal only.

**`callisto-conventional` (§C.10)** — 4 decisions: case-sensitive type-token classification;
simplified footer-block detection; pre-major policy applied to the aggregate; the pre-major
gate is SemVer-only by caller contract.

**`callisto-changelog` (§CL.10)** — 6 decisions: `ChangelogInput` is richer than `BumpReason`;
one render function, three consumers; `ChangeSource`'s six variants; the bullet-text templates;
`Package.changelog: None` means opt-out; `extract_section` as `changelogSection`'s production
path, with the field made optional.

**`callisto-graph` (§G.16)** — 36 decisions. The ones most worth a second look:

| # | Section | Decision |
|---|---|---|
| G3 | §G.1.7 | Zero-moon enforced by `xtask dep-audit` over `cargo metadata`'s resolve graph. |
| G4 | §G.1.7 | §13 inv. 27 also enforced by removing the `callisto-cli → callisto-manifests` edge. |
| G5 | §G.2.3 | `pnpm-workspace.yaml` is a fourth workspace-root marker. |
| G6 | §G.3.2 | `toposort` excludes `Dev`/`Peer` edges. |
| G10 | §G.5.2 | `moon.yml`'s `extensions.callisto` block has highest precedence, read as plain YAML. |
| G11 | §G.5.3 | `pre-major-inference` is three-valued (`off`/`conservative`/`conservative-feat`). |
| G13 | §G.6.4 | `SeverityInference` is graph-owned; `CommitInference` lives in `callisto-graph` behind an optional `inference` feature. |
| G14 | §G.6.6 | An explicit `none` changeset entry counts as a naming event for linked joint detection. |
| G15 | §G.6.7 | A linked joint release forces a shared maximum **version**, not just severity. |
| G16 | §G.7.1 | `mode = "always"` does not trigger peer escalation. |
| G17 | §G.7.1 | `Coverage::Unknown`'s behaviour under each mode; never rewritten. |
| G18 | §G.7.5 | The fixed-group severity union is maintained by `raise`, closing the peer-escalation hole in §7.1's pre-step-only claim. |
| G19 | §G.7.6 | The convergence bound is `4n + 1`, derived from the severity lattice's height. |
| G20 | §G.7.7 | Round-trip fidelity is verified by re-parse plus re-coverage. |
| G21 | §G.8.3 | The aligned-base fallback for a fixed group with no released member. |
| G22 | §G.8.4 | napi drift compares target triples via a localized table. |
| G23 | §G.10.2 | §7.6's mutation ordering lives in `callisto-graph`. |
| G24 | §G.11 | `commands::init` exists, with the pinned signature. |
| G28 | §G.7.3 / §G.10.1 | Version and dependency writes carry **typed** targets (`VersionWriteTarget`, `DepWriteTarget`), so `[workspace.package].version` and a root package's own `[package].version` cannot be confused. |
| G30 | §G.10.2 | `ApplyOptions::transient` — snapshot must not delete changesets or write changelogs. |
| G31 | §G.11 | `initialVersions` is keyed by ecosystem-native name, computed in the core. |
| G33 | §G.11 | The snapshot version is `0.0.0-{tag}-{sha7}`, workspace-wide. |
| G34 | §G.11 | `compose-pr-body` computes real prospective versions. |
| G36 | §G.7.4 | `bump_target` delegates to `fixed_group_target`, which is what makes the new-member force-set real. |

**`callisto-cli` (§CLI.10)** — 14 decisions: the global-flag surface; central stderr
forwarding; `find_workspace_root`/`IgnoreWalkLocator::new`; 2-space stdout JSON;
`add`'s ad-hoc envelope and CLI-owned filename generation; feature-gated inference selection;
`pre exit` skipping `Workspace::load`; `compose-pr-body`'s input flags; `tag --plan`; stderr-only
errors; the `wrapper` lib feature; `--strict`/`--strict-graph` on `status`/`version`;
`--allow-empty-changesets`; and `schemaVersion` on `add`/`pre`'s ad-hoc envelopes.

**`callisto-moon` (§MO.11)** — 7 decisions: the pinned moon range; near-empty
`define_extension_config` schema; shared clap grammar *and* renderers; manifest-presence
ecosystem detection; `IdentityResolver` reuse; heuristic host-failure classification; the
feature-gated `callisto-cli` edge.

**`callisto-fixtures`** — 2 decisions, both introduced during assembly and listed in §11.2
(the license/dependency correction, and the `graph` feature split).

**`callisto-vcs` (§V.9)** — 0 decisions: written directly from shipped source rather than
from a design-doc gap, so no open reading needed resolving.

### 11.2 Reconciliations made while assembling this document

Each row is a case where two independently-drafted crate specs described the same thing
differently, or where a signature in one draft was unimplementable given another's constraints.
The "resolved to" column is what this document says; the rule applied throughout is the one the
assembly brief states — **shared types take `callisto-model`'s shape, since everything depends
on it** — with "the crate that owns the concept wins" as the tiebreaker where `callisto-model`
has no stake.

| # | Conflict | Resolved to | Where |
|---|---|---|---|
| R1 | `callisto-format`'s draft made `Severity` its own and gave the crate **zero** `callisto-*` dependencies; `callisto-model`'s draft fixed the edge as `callisto-format → callisto-model` and listed `Severity` among its own types. Cyclic if both hold. | `Severity`, `SeverityParseError`, and its `FromStr`/`Display` live in `callisto-model` (§M.5); `callisto-format` depends on `callisto-model` and re-exports `Severity` for convenience. The zero-dependency claim is narrowed to "no dependency on any *behavioural* callisto crate." | §M.4.5, §F.2 |
| R2 | `callisto-format`'s `bump_version` took `&semver::Version` and was infallible; `callisto-model`'s `Version` is grammar-tagged. | `bump_version(&Version, Severity) -> Result<Version, BumpError>`, with one grammar-precondition error variant, plus the `Versioning` trait §7.7/§M.15 require. | §F.6 |
| R3 | `PreState::initial_versions` was `IndexMap<String, semver::Version>`; `PreJsonError::InvalidInitialVersion` sourced `semver::Error`. | `IndexMap<String, callisto_model::Version>` and `VersionParseError`. Keys stay `String` — name→identity resolution is not `callisto-format`'s. | §F.7 |
| R4 | `callisto-cli`'s draft called `callisto_format::changeset::write(&dir, &entries, &summary)` — a filesystem-touching API in a crate that is filesystem-free. | `callisto-cli` builds a `Changeset`, calls `write_changeset` for the string, and does its own filename generation and `fs::write`. | §CLI.6.1, §F.3 |
| R5 | Same for `PreState::enter(&root, …)`/`exit(&root)`. | Pure `PreState::entering`/`exiting` constructors in `callisto-format`; the file read/write is `callisto-cli`'s. | §F.7, §CLI.6.4 |
| R6 | `callisto-manifests::round_trip` and `callisto-graph::rewrite_spec` each described the whole spec-rewriting operation, including operator/precision preservation and the round-trip check. | Split by concern: `round_trip` owns the per-ecosystem **grammar**; `rewrite_spec` owns the **policy** — the `preserve-npm-ranges` short-circuit, the re-parse + re-coverage verification, and the `None` → `Diagnostic` mapping. | §CM.3, §G.7.7 |
| R7 | `callisto-cli` constructed `callisto_conventional::CommitInference` as a `SeverityInference` impl, but `callisto-conventional` cannot see that trait (no graph dependency) and `callisto-graph`'s draft said it must not depend on `callisto-conventional`. | `callisto_graph::infer::CommitInference`, behind `callisto-graph`'s optional `inference` feature. Both wrappers stay thin; exactly one adapter exists. | §G.6.4, §CLI.6.3.1 |
| R8 | `pre-major-inference` was one-valued in `callisto-conventional`'s reading of §14 and three-valued in `callisto-graph`'s. | Three-valued (`off`/`conservative`/`conservative-feat`), mapping onto `PreMajorInferencePolicy`'s two independently-gated bools — the only shape that makes §7.1's "separately gated" expressible. | §G.5.3, §C.4 |
| R9 | `callisto-moon`'s draft said it does **not** depend on `callisto-cli`, then twice specified an `args-only`-featured dependency on it. | The edge exists, narrowly, and the feature is renamed `wrapper` because it must also carry the human renderers (§13 inv. 28's attribution lines are a user-visible contract that must not drift between wrappers). | §MO.1, §MO.2.3, §CLI.8 |
| R10 | `MoonProjectLocator` needed `PackageId`s; one draft had it depend on `callisto-manifests` directly, another had it call a free `callisto_graph::identity::resolve_package_id(root, eco, &OpenContext)` — which would still force it to name a `callisto-manifests` type. | `callisto_graph::identity::IdentityResolver`, a struct constructed once per invocation. `callisto-moon` names no `callisto-manifests` type; the `callisto-manifests` edge remains only for feature forwarding. | §G.4.2, §MO.4.3 |
| R11 | `callisto-cli`'s draft defined a local `ConfigProvenanceLookup` trait as a stand-in because `ResolvedConfig`'s API was unpinned. | `ResolvedConfig::provenance` / `rendered_value` are pinned in §G.5.4; the stand-in trait is dropped. | §G.5.4, §CLI.5.3 |
| R12 | `commands::init` was assumed by `callisto-cli` and `callisto-moon` but absent from `callisto-graph`'s command list and milestone table. | Added, v0.1, with a pinned signature and `InitOptions`. | §G.11, §G.14 |
| R13 | `callisto-conventional` used `&dyn CommandRunner`; `callisto-graph` used generic `R: CommandRunner`. | Both stand. `CommandRunner` is documented as deliberately dyn-compatible so `&R` coerces at the boundary; neither form is privileged. | §M.10, §C.5 |
| R14 | `Diagnostic::path` for §6.3's empty-changeset check needs a changeset path, but `Changeset` carries none. | `callisto-graph::LoadedChangeset { path, id, changeset }` — the graph-side wrapper that attaches the two identity fields `callisto-format` deliberately omits. | §G.6.1, §F.5.1 |
| R15 | `callisto-changelog`'s `ChangeSource::Commit { sha: String }` vs. `callisto-model`'s `CommitSha` newtype. | `CommitSha`, with `short()` supplying the 7-character rendering. | §CL.3, §M.2 |
| R16 | `callisto-fixtures`'s draft claimed MIT/Apache-2.0 and a `callisto-model`-only dependency, while also holding the in-memory `DependencyResolver` (a `callisto-graph` trait) and every crate's corpus. | AGPL-3.0, `publish = false`, with a `graph` feature that adds the five behavioural crates. Only `callisto-format` and `callisto-model` enable default features; `callisto-format`'s is what keeps the normal-dependency graph acyclic. Both wrappers enable `graph`, since their own fixtures need `ReplayCommandRunner`. | §CF.2 |
| R17 | Per-crate fixture *assertions* were described as living inside `callisto-fixtures` (including `callisto-cli`'s `main()` tests and `callisto-moon`'s locator tests), which would require this crate to depend on both wrappers that dev-depend on it. | Corpus and doubles here; assertions in each crate's own `tests/`. §CF.6 is the index. | §CF.2, §CF.6 |
| R18 | `GraphError` needed to carry `ChangelogError` and `ConventionalError`, which §M.13.3's pinned list did not include. | Two more transparent `#[from]` wrappers alongside §G.12's `Config`, recorded in §M.13.3 so the two crates' specs agree on the vocabulary. | §M.13.3, §G.12 |
| R19 | `callisto-format`'s draft normalized CRLF; `callisto-manifests`'s preserved it. Read together this looks inconsistent. | Both stand, and the asymmetry is now stated with its reason: callisto *authors* changeset files (so it must match `@changesets/cli` byte-for-byte, which is LF), and *edits* `package.json` (whose formatting belongs to the user). | §F.5.3, §CM.5.1 |
| R20 | `PreJsonError::Malformed` held a `serde_json::Error`, which is not `Clone + PartialEq` — breaking §1.5-of-this-document’s fixture-comparability rule that every other error type in the workspace follows. | Carries a rendered `message: String`. | §F.7 |
| R21 | §G.4.4's edge extraction called `entry.spec.is_workspace_inherited()`, a method no `DepSpec` has and none can usefully have (§CM.4.2 requires the *resolved* spec to be yielded, so an inherited entry's `DepSpec` is byte-identical to a locally-declared one). | `DependencyEntry::inherited: bool`, carried onto `DepEdge::inherited`. Provenance is a property of the record, not of the requirement. | §M.7.3, §CM.4.2, §G.4.4 |
| R22 | §G.1.3's cross-crate signature block disagreed with **every** section it summarised: `render_section` infallible vs. §CL.5's `Result`; a pure-string `prepend(existing, section) -> String` vs. §CL.6's file-writing `prepend(root, changelog_path, display_name, rendered) -> Result<(), _>`; `versioning_for -> &'static dyn` vs. §F.6.1's `Option`; a `Versioning` trait with `bump_prerelease` that §F.6.1 did not declare; a `WorkspaceCargoResolver::inherited` that §CM.4.4 did not have. | The block is corrected against each section and now annotates every signature with the section that owns it, since a drift-prevention copy that has itself drifted is worse than no copy. `bump_prerelease` is added to the **real** trait with a real algorithm (§F.6.3) rather than deleted — §8's pre-release counter had no owner anywhere before this pass. `inherited` becomes `WorkspaceInheritance::inherited`, the type graph construction actually holds. | §G.1.3, §F.6.1, §F.6.3, §CM.4.4, §CL.5, §CL.6 |
| R23 | Four sites described Cargo `[workspace.dependencies]` inheritance incompatibly: §CM.4.2's "transparent resolution", §CM.4.4's resolver with no read accessor, §G.4.4's nonexistent method, and §G.10.2's apply steps routing inherited writes through the member's `Manifest` methods. | One story: `DependencyEntry::inherited` signals it; `WorkspaceInheritance::inherited` resolves it, kind-preservingly; `DepWriteTarget`/`VersionWriteTarget` carry the typed write target into the plan; §G.10.2 dispatches on those, so an inherited write can only reach `WorkspaceCargoResolver`. | §M.7.3, §CM.4.2, §CM.4.4, §G.4.4, §G.7.3, §G.10.1, §G.10.2 |
| R24 | `callisto pre enter` was specified to snapshot on-disk versions it had no way to read — `Package` has no version field (M6), `callisto-cli` cannot depend on `callisto-manifests` (G4), and `Workspace` exposed no version map — and to key `initialVersions` by `PackageId::display_name()`, which `@changesets/cli` would never write. | `Workspace::base_versions`/`initial_versions`/`pre_json_key` in `callisto-graph` (which *may* depend on `callisto-manifests`), keyed by ecosystem-native name, npm-preferred for Case D. `PreState::entering` takes an iterator so no wrapper needs `indexmap`. | §G.11, §F.7, §CLI.6.4 |
| R25 | `callisto snapshot` called `apply_version_plan` unconditionally, which always deletes consumed changesets and prepends changelogs (§7.6 steps 7–8) — data loss for a command §8 describes as writing "uncommitted, untagged." | `ApplyOptions::transient`, suppressing steps 7, 8 and 11; `plan_snapshot` additionally leaves the corresponding plan fields empty. | §G.10.2, §CLI.6.6 |

### 11.3 What is still open

Three things this document deliberately does not resolve, each because resolving it now would
be guessing:

1. **The moon compatibility range's actual numbers** (§MO.0) — a placeholder until checked
   against moon's changelog at implementation time. The *mechanism* (a checked range, verified
   once per invocation at `register_extension`) is what this spec pins.
2. **Go's `current_version`/`write_version` semantics** (§CM.7) — §7.7 already flags this as "a
   real deviation… should be documented as such whenever Go support ships." This spec inherits
   the flag rather than resolving it, since resolving it is demand-gated design work §2.2 asks
   not to do prematurely.
3. **`ParseError::DuplicateEntry`** (§F.5.5) — flagged for re-litigation if a real
   `@changesets/cli`-authored fixture is ever found that relies on YAML duplicate-key
   tolerance. Until then, P5's refuse-rather-than-guess applies.


## 12. `callisto-vcs`

**Purpose.** One `GitDataSource` trait, two backends, and a selector that picks between them
per operation so every other crate that needs Git — `callisto-graph` (tags, changed-since,
publish SHAs) and `callisto-conventional` (commit-severity inference windows) — calls one API
regardless of whether native `gix` is available on the running target.

**License:** MIT/Apache-2.0 (§16 — despite sitting below AGPL `callisto-graph` in the
dependency graph, this crate depends on nothing but `callisto-model`, so nothing AGPL leaks
into it; the permissive/AGPL boundary is a one-way constraint on what a permissive crate may
depend on, not on who may depend on a permissive crate).
**Milestone:** v0.1 (§17 — repository discovery and tag/commit reads are load-bearing for
`status`'s `last_tag_for`/`changed_since_last_tag` from the first shipped milestone).

### V.0 Dependencies and boundaries

| Edge | Kind | Why |
|---|---|---|
| `callisto-vcs → callisto-model` | normal | `ApplyPermit`, `CommandError`, `CommandRunner`, `CommitRecord`, `CommitSha`, `CommitWalkError`, `CommitWalker`, `TagName` |
| `gix` | normal, `cfg(not(target_arch = "wasm32"))` only | native backend (§V.4) |
| `globset` | normal | tag-glob matching, shared identically by both backends (§V.4, §V.5) |
| `dunce` | normal | UNC-prefix-stripped canonicalization before `gix::discover` (§V.4) |
| `callisto-model` (`test-util` feature) | **dev** | `ApplyPermit::force_for_tests` — tests mint a write permit directly, with no dry-run flag to consult |
| `tempfile` | **dev** | temporary repository fixtures built with the real `git` binary (§V.8) |

**Deliberately absent:** `callisto-graph`, `callisto-manifests`, `callisto-format`. This crate
has no concept of a `Package`, a manifest, or a changeset — it answers exactly two kinds of
question, "what does history/refs say" and "write this ref," using only `CommitSha`/`TagName`
as vocabulary. `callisto-moon` never depends on this crate at all (not even for the shell
backend): gix's mmap-based object reads hit `ENOSYS` under `wasm32-wasip1` (confirmed by a
2026 probe, `ARCHITECTURE.md`'s §"In-Process VCS Engine"), so `GitRepository::discover` is
`cfg`-gated out entirely on that target and would
contribute nothing `callisto-moon` could use — the WASM extension's own exec seam calls
`CommandRunner` directly against moon's `exec_command` host function instead, without going
through this crate's `ShellGit` wrapper.

**What this crate is not responsible for:** deciding *which* ref format a tag name should take
(`tag_template` resolution is `callisto-model`'s `last_tag_for`, §M.9.4, plus
`callisto-graph`'s `git tag --list` glob execution, §G.9.1 — this crate only ever receives an
already-resolved literal name or glob string), and deciding *when* a tag should be created
(§9.1's "never at `version` time" rule is enforced by callers; `create_tag` will happily create
a tag the instant it's called).

### V.1 Module layout

```
callisto-vcs/
├── Cargo.toml   # deps: callisto-model, thiserror, miette, globset, dunce; gix (non-wasm32
│                  target only). dev-deps: callisto-model (test-util), tempfile.
└── src/
    ├── lib.rs     # VcsError, GitCommit alias, GitVcsProvider, GitDataSource, GitRepository,
    │                CommitWalker bridge (§V.7)
    ├── access.rs  # GitAccess — per-operation native/shell selection (§V.6)
    └── shell.rs   # ShellGit — CommandRunner-shelled backend (§V.5)
```

### V.2 Errors — `VcsError` (`lib.rs`)

```rust
#[derive(Clone, Debug, thiserror::Error, miette::Diagnostic, PartialEq, Eq)]
#[non_exhaustive]
pub enum VcsError {
    #[error("failed to discover Git repository at `{path}`: {message}")]
    #[diagnostic(code(E050), help("Ensure target directory is inside a valid Git repository."))]
    RepoNotFound { path: PathBuf, message: String },

    #[error("git error: {0}")]
    #[diagnostic(code(E051))]
    Git(String),

    #[error("reference `{ref_name}` was not found")]
    #[diagnostic(code(E052), help("Check if reference or tag exists in local or remote Git refs."))]
    RefNotFound { ref_name: String },

    #[error("tag glob pattern `{pattern}` is not a valid glob: {message}")]
    #[diagnostic(code(E053), help("Fix the glob syntax or use a literal tag name."))]
    InvalidGlob { pattern: String, message: String },

    /// Wraps a `CommandError` surfaced by the shell backend — e.g. `git` itself couldn't be
    /// spawned. Kept `transparent` so callers that only care about the underlying
    /// `CommandError` can match through it regardless of which backend served the call.
    #[error(transparent)]
    Command(#[from] CommandError),
}
```

`callisto-graph` wraps this transparently (`GraphError::Vcs`, §error-taxonomy — no dedicated
E-code of its own; the four codes above are `VcsError`'s own and survive unchanged through the
transparent wrap). `E050`–`E053` is this crate's own contiguous block, chosen not to collide
with `callisto-model`'s or `callisto-graph`'s ranges (§error-taxonomy).

**`RefNotFound` is not always an error at the trait level.** `GitDataSource::resolve_commit`
returns `Ok(None)` for an unresolvable ref — a caller-visible "no bound" signal, not a failure.
`VcsError::RefNotFound` exists for the one place resolution failure *is* fatal:
`GitDataSource::commits_since`'s `since_ref: Some(r)` where `r` fails to resolve (§V.4) — the
distinction matters because a caller that explicitly bounded its walk must never silently fall
back to unbounded history (§V.4's regression note).

### V.3 `GitDataSource` — the unified access trait (`lib.rs`)

```rust
pub type GitCommit = callisto_model::CommitRecord;

pub trait GitDataSource {
    fn head_sha(&self) -> Result<CommitSha, VcsError>;
    fn list_tags(&self, glob: Option<&str>) -> Result<Vec<TagName>, VcsError>;
    fn resolve_commit(&self, refname: &str) -> Result<Option<CommitSha>, VcsError>;
    fn commits_since(&self, since_ref: Option<&str>, pathspecs: &[PathBuf])
        -> Result<Vec<GitCommit>, VcsError>;
    fn create_tag(&self, name: &str, target_sha: &CommitSha, message: Option<&str>,
        permit: &ApplyPermit) -> Result<(), VcsError>;
    fn create_floating_major(&self, major_name: &str, target_sha: &CommitSha,
        permit: &ApplyPermit) -> Result<(), VcsError>;
}
```

`GitCommit` is a type alias for `callisto_model::CommitRecord`, not a redeclaration — a commit
crosses the `CommitWalker` seam (§V.7, Layer 1) without a conversion, and there is exactly one
definition of "what a commit looks like" in the workspace. Three implementors: `GitRepository`
(§V.4), `ShellGit` (§V.5), and `GitAccess` (§V.6, the one callers actually construct).
`create_tag`/`create_floating_major` take an `ApplyPermit` (§M.10) because they write a ref; a
dry run mints no permit and so cannot reach either call.

`list_tags`'s `glob` parameter is matched with `globset::Glob` — **identically** by both
backends (§V.4 filters `gix`'s own reference iterator locally; §V.5 always fetches the
*unfiltered* `git tag --list` and filters locally too, deliberately never delegating to `git
tag --list <pattern>`'s own, different glob dialect) — so tag selection is byte-identical
regardless of which backend served the request. A pattern that fails to compile is
`Err(VcsError::InvalidGlob)`, never a silent "match everything," since a malformed
`tag_template`-derived glob silently matching every tag in the repo would let `last_tag_for`
(§M.9.4) pick an unrelated package's tag.

### V.4 `GitRepository` — native `gix` backend (`lib.rs`)

```rust
pub struct GitRepository { /* wraps gix::Repository, cfg(not(wasm32)) only */ }

impl GitRepository {
    pub fn discover(path: impl AsRef<Path>) -> Result<Self, VcsError>;
    pub fn head_sha(&self) -> Result<CommitSha, VcsError>;
    pub fn list_tags(&self, glob_pattern: Option<&str>) -> Result<Vec<TagName>, VcsError>;
    pub fn resolve_commit(&self, refname: &str) -> Result<Option<CommitSha>, VcsError>;
    pub fn commits_since_with_pathspec(&self, since: Option<&CommitSha>, pathspecs: &[PathBuf])
        -> Result<Vec<GitCommit>, VcsError>;
    pub fn create_tag(&self, name: &str, target_sha: &CommitSha, message: Option<&str>,
        permit: &ApplyPermit) -> Result<(), VcsError>;
    pub fn create_floating_major(&self, major_name: &str, target_sha: &CommitSha,
        permit: &ApplyPermit) -> Result<(), VcsError>;
}
```

`discover` runs `path` through `dunce::canonicalize` first (falling back to the raw path if
canonicalization itself fails) before handing it to `gix::discover` — the same UNC-stripping
concern as every other canonicalization site in the workspace. Every method is `cfg`-split
in its body: the non-`wasm32` half does the real `gix` call; the `wasm32` half is either an
always-`Err` (`discover`, `create_tag`, `create_floating_major` — no meaningful degraded
behavior for a write or for the root discovery call itself) or a harmless always-empty/`Ok(None)`
(`list_tags`, `resolve_commit` — a caller on that target never actually reaches these, since
`discover` already failed upstream, but the bodies stay total rather than panicking).

**`resolve_commit` treats an unresolvable ref as `Ok(None)`, not `Err`** — chained `let...else`
steps (rev-parse → object → peel-to-commit) each degrade to `None` on failure. This is a
deliberate two-tier contract with `commits_since`: an *implicit* absence of a bound
(`since_ref: None`) and an *explicit* bound that fails to resolve are different situations, and
only `GitDataSource::commits_since` (not `resolve_commit` itself) is where the second case
becomes `Err(VcsError::RefNotFound)`.

**Regression fixed by this shape: no silent unbounded fallback.** An earlier version of this
method resolved `since_ref` *internally* and, on a resolution failure, fell through to walking
the entire history unbounded — a correctness bug (already-released commits could re-surface
into changelog/severity inference) masquerading as graceful degradation. The fix routes
`since_ref` resolution through `resolve_commit` and an explicit `Ok(None) =>
Err(RefNotFound)` step in `commits_since`, so an unresolvable *explicit* bound is always
surfaced as an error; `since_ref: None` (no bound requested at all) is unaffected.

`commits_since_with_pathspec` walks `gix::Repository::rev_walk` from `HEAD`, excluding every
SHA reachable from `since` (computed as its own full walk, collected into a `HashSet`, so
membership is checked with `continue` rather than terminating the outer walk with `break` — a
topological walk can visit `since` before it has emitted every commit on a branch that
diverged *before* `since`, and a `break` would silently drop those still-queued commits; this
was a real bug, pinned by `test_commits_since_with_pathspec_includes_pre_tag_branch_commits`).
Merge commits (more than one parent) are always skipped, matching `git log --no-merges`.
Path-scoping diffs each commit's tree against its first parent (or the empty tree, for a root
commit) with `track_rewrites(None)` — a rename is reported as a separate `Deletion`+`Addition`
rather than one `Rewrite`, and either half matching a pathspec counts as "touched," which is
also why a file moved *out of* a matching directory still shows that commit as touching it.
Commit message CRLF sequences are normalized to `\n` in both `summary` and `body`.

### V.5 `ShellGit` — `CommandRunner`-shelled backend (`shell.rs`)

```rust
pub struct ShellGit<'r> { /* runner: &'r dyn CommandRunner, root: PathBuf */ }

impl<'r> ShellGit<'r> {
    pub fn new(runner: &'r dyn CommandRunner, root: impl Into<PathBuf>) -> Self;
}
```

Implements `GitDataSource` by shelling exactly the `git` subcommands each operation needs,
consolidating five previously independent hand-rolled fallbacks (`callisto-graph`'s
`changed.rs`, `tags.rs`, `commands/tag.rs`, `aggregate.rs`, and `callisto-conventional`'s
`window.rs`). Works on every target, including `wasm32`, since it never touches `gix`.

| Operation | Shell command |
|---|---|
| `head_sha` | `git rev-parse HEAD` |
| `list_tags` | `git tag --list`, filtered locally with `globset` (§V.3) |
| `resolve_commit` | `git rev-parse --verify --quiet <ref>^{commit}` — non-zero or empty stdout is `Ok(None)`, never `Err` |
| `commits_since` | `git log --no-merges --format=<RS>%H<US>%B <since>..HEAD\|HEAD [-- <pathspecs>]` |
| `create_tag` | `git tag [-a -m <message>] -- <name> <sha>` |
| `create_floating_major` | `git tag -f -- <major_name> <sha>` |

`commits_since` deliberately does **not** pre-resolve `since_ref` with a separate `rev-parse`
round-trip: an unresolvable `since_ref` already makes `git log <since_ref>..HEAD` itself exit
non-zero, which the method surfaces as `Err(VcsError::Git(..))` exactly like any other `git
log` failure — one shell call either way, and the same no-silent-unbounded-walk guarantee
§V.4 documents for the native backend, achieved by a different mechanism (a failing command
instead of a resolved-then-checked ref).

`--format=` uses two control characters as delimiters, never present in ordinary commit text:
`\u{1e}` (record separator) immediately before each commit's SHA, and `\u{1f}` (field
separator) between the SHA and the raw `%B` message body. This makes a commit message
containing literal newlines — or even one that happened to contain the record separator
itself, if a commit author somehow typed it — unambiguous to split back into records, which a
newline- or blank-line-based delimiter could not guarantee. Parsing then splits each raw
message on its *first* blank line into `summary`/`body`, mirroring how `gix`'s own
commit-message parsing splits title from body on the native path, so both backends hand
callers byte-identical shapes. `--` before pathspecs and before `name`/`target_sha` in every
tag-writing command marks the end of option parsing, so a value that happens to start with `-`
(defended against upstream by `is_valid_git_ref_name`, but defended here too) is never
misread as a flag.

### V.6 `GitAccess` — backend selection (`access.rs`)

```rust
pub struct GitAccess<'r> { /* native: Option<GitRepository>, shell: ShellGit<'r> */ }

impl<'r> GitAccess<'r> {
    pub fn discover(root: impl AsRef<Path>, runner: &'r dyn CommandRunner) -> Self;
}
```

The type every production caller outside this crate actually constructs — exclusively within
`callisto-graph` (§G.9, and the command handlers §G.11 documents). `callisto-conventional`
touches `GitAccess` only inside its own test code — its production `infer_severity` and
`fetch_commits` take `&dyn CommitWalker` directly and link against no VCS crate at all, per
their own doc comments in source (`crates/callisto-conventional/src/{infer,window}.rs`; §C.5
and §C.7 of this document still show an older `CommandRunner`-based signature for both and
are tracked as stale — see the backlog item this finding produced). `discover`
never fails: it attempts native `gix` discovery and keeps the `Option<GitRepository>` result
either way, while unconditionally preparing a `ShellGit` against the same root as the
fallback (or sole) backend. A discovery failure — not a repo, or `wasm32` where `gix` is
excluded from the dependency set entirely — just means every operation on this `GitAccess`
runs through the shell.

**Fallback policy differs by operation category, deliberately:**

- **Reads** (`head_sha`, `list_tags`, `resolve_commit`, `commits_since`): fall back to the
  shell backend whenever native `gix` errors for *any* reason — failed discovery as well as a
  discovered repo's own operation failing. Retrying a read through the shell can only help: at
  worst it fails too and the error propagates from there instead.
- **Writes** (`create_tag`, `create_floating_major`): fall back to the shell *only* when
  native `gix` was never available to begin with (discovery failed). If a repo *was*
  discovered, its result — success or failure — is authoritative and returned as-is,
  never retried through the shell. Retrying a failed mutation through a second, different
  code path risks masking a genuine failure (e.g. "tag already exists") or double-applying a
  mutation the first attempt partially completed — a risk read-only retries don't carry.

`GitAccess` implements `GitDataSource` by trying `self.native`'s corresponding method first
(reads: `if let Ok(..) = ...`, falling through on any `Err`; writes: an unconditional early
`return` when `self.native` is `Some`, regardless of whether the call itself succeeds) and
falling back to `self.shell` only in the cases the policy above allows.

### V.7 `CommitWalker` integration — bridging to Layer 1 (`lib.rs`)

```rust
impl From<VcsError> for CommitWalkError { /* narrows to the Layer 1 vocabulary, below */ }
```

`callisto_model::CommitWalker` (`callisto-model/src/commit.rs`) is a Layer 1 trait — it must not know `VcsError` exists,
since `callisto-model` depends on no `callisto-*` crate (§M.5). This crate bridges the gap:
`CommitWalkError::Command` and `CommitWalkError::RefNotFound` survive the narrowing as
themselves (the two distinctions Layer 1 callers branch on); every other `VcsError` variant —
`RepoNotFound`, `Git`, `InvalidGlob` — is gix- or repository-specific with no Layer 1
equivalent, so it collapses into `CommitWalkError::Backend { message }` carrying the original
`Display` rendering, losing nothing a user would see. `GitAccess`, `GitRepository`, and
`ShellGit` each get a `CommitWalker` impl whose body is identical (delegate `commits_since`,
map the error through the `From` above) — written via a macro rather than a blanket `impl<T:
GitDataSource> CommitWalker for T`, since `CommitWalker` is foreign to this crate and a
blanket impl over an uncovered type parameter is forbidden by Rust's orphan rules.

### V.8 Fixture obligations

Per §12.6's "broader than JSON shape alone" (this crate has no JSON output of its own — its
data feeds `callisto-graph`'s tag/commit logic, not stdout directly):

1. **Backend-parity corpus.** Every `GitDataSource` operation exercised against a real
   temporary repository (built with the real `git` binary, not mocked) through both
   `GitRepository` directly and `ShellGit` against a `CommandRunner` shelling that same real
   `git`, asserting identical results — this is what makes §V.3's "byte-identical regardless
   of backend" claim a tested property, not an aspiration.
2. **`GitAccess` selection corpus.** A poisoned `CommandRunner` (panics/errors on any
   invocation) proves a real-repo read never touches the shell; a non-repo root with a
   call-counting `CommandRunner` proves exactly one shell call serves the read fallback; a
   real repo with a failing write (`create_tag` on an already-existing name) proves the
   failure propagates without a rescue attempt through the poisoned shell.
3. **`commits_since` regression corpus.** The pre-tag-branch-commit scenario (§V.4) and the
   ref-not-found-must-error scenario (§V.4, §V.5), each proven independently through both
   backends.
4. **Glob-parity corpus.** The same malformed glob pattern against both backends, asserting
   `Err(VcsError::InvalidGlob)` from each — not "some" error, the specific variant, since a
   caller pattern-matches on it.
5. **`wasm32-wasip1` build check.** This crate must compile for that target (native `gix`
   `cfg`'d out, `GitRepository::discover` always `Err`) — no runtime test suite, since
   `callisto-moon` never constructs `GitRepository`/`GitAccess` directly (§V.0); a build
   failure here would still indicate a real problem (an accidental non-`cfg`-gated `gix` call).

### V.9 Index of `[SPEC DECISION]` flags

None. Every shape and policy in this section — the four `VcsError` codes, the read/write
fallback-policy split, the `CommitWalker` narrowing rule — is either pinned directly by
`00-design.md` §9.4/§13 or is existing, shipped behavior with no open reading of the design
doc left to resolve; this crate's section was written directly from source (§1's "traced back
to source" standard) rather than from a design-doc gap requiring a documented choice.

---

## 13. Callisto v1.0 Production Hardening & Moon Alignment

This section documents the formal specification additions for Callisto's v1.0 initial release, incorporating architectural patterns from `moonrepo` (`moon`) and modern Rust CLI engineering standards.

### 13.1 Graph Engine Specification (`petgraph` & Cycle Extraction)
1. **Graph Backing**: `callisto-graph`'s workspace graph MUST be backed by a directed graph structure (`petgraph::graph::DiGraph<PackageId, DepEdge>`).
2. **Cycle Diagnostics**: When graph traversal encounters a circular dependency, the solver MUST run Tarjan's Strongly Connected Components algorithm (`petgraph::algo::tarjan_scc`) to isolate the cyclic nodes and construct a `Diagnostic` with code `callisto::circular_dependency` detailing the exact cycle path (`pkg-a → pkg-b → pkg-a`).

### 13.2 Schema Export Grammar (`schemars`)
1. **Schema Derivation**: All serializable configuration and state types in `callisto-model` (`CallistoConfig`, `PreState`, `Changeset`, `Severity`, `Version`) MUST derive `schemars::JsonSchema`.
2. **Schema CLI Subcommand**: `callisto-cli` MUST expose a `callisto schema` subcommand that outputs JSON Schema (draft-07) payloads for integration with IDE language servers (`even-better-toml`, `yaml-language-server`).

### 13.3 Atomic File Write Engine
1. **Mutation Safety**: All manifest and changeset writes in `callisto-manifests` and `callisto-graph` MUST write to a temporary file (`tempfile::NamedTempFile`) created within the target file's parent directory.
2. **Atomic Swap**: Upon successful write and flushing, the temporary file MUST be atomically persisted over the target path using `fs::rename` semantics.

### 13.4 CLI Diagnostics & Safety Flags
1. **Rich Diagnostic Cards**: `callisto-cli` MUST implement `miette::Diagnostic` on `CliError` to format errors with colorized snippets, error codes (`callisto::*`), and actionable remediation tips.
2. **Pipe-Safe Coloring**: Terminal output MUST use `anstream` ANSI stream styling that automatically suppresses ANSI codes when stdout/stderr is redirected to a non-TTY stream or file pipe.
3. **Dry-Run Diff Preview**: Commands that perform manifest or changeset mutations (`version`, `snapshot`, `pre`) MUST accept a `--dry-run` flag. When active, mutations MUST NOT be committed to disk; instead, unified git-style diffs (using `similar`) MUST be emitted to stdout.

### 13.5 WASM Target & Plugin PDK Protocol
1. **Target Capability**: `callisto-moon` MUST be compilable to `wasm32-wasip1` using `extism-pdk` to allow Moon v1/v2 to execute Callisto inside its native WebAssembly sandbox.

### 13.6 Native VCS Engine Architecture (`callisto-vcs` & `gix`)

**Purpose.** In-process Git operations powered by `gix` (gitoxide), eliminating subprocess
fork/exec overhead for repository discovery, ref matching, commit history revwalks, tag
filtering, and HEAD SHA retrieval where available, with a `CommandRunner`-shelled fallback
everywhere else. Full type signatures, the read/write fallback-policy split, and fixture
obligations live in **§V (`callisto-vcs`, §12)** — this subsection states the two MUST-level
requirements only; it is not a second, independent sketch of the crate's shapes.

1. **In-Process Git Engine**: Repository discovery (`GitRepository::discover`), ref resolution
   (`resolve_commit`), tag enumeration (`list_tags`), commit history revwalks
   (`commits_since_with_pathspec`), and HEAD SHA retrieval (`head_sha`) MUST be encapsulated
   within `callisto-vcs` using pure-Rust `gix` (§V.4) — never called directly by
   `callisto-graph` or `callisto-conventional`.
2. **Subprocess Fallback**: Every caller MUST reach Git through `GitAccess::discover` (§V.6),
   not through `GitRepository` or `ShellGit` directly — `GitAccess` is what applies the
   read/write fallback-policy split (§V.6) that makes "seamlessly falls back to `CommandRunner`
   subprocess calls when `gix` is unavailable" true without each call site re-implementing the
   policy itself.

### 13.7 GitHub Actions Workflow & Moon Alignment (`callisto-action`)
1. **Action Architecture**: Callisto release orchestration in CI MUST be composed as a CLI consumer using `callisto-cli` binary calls, `gh` CLI for Pull Requests, and `moon run :publish` for multi-ecosystem package publishing.
2. **Composite Action**: `.github/actions/setup-callisto` provides automated binary caching and installation for GitHub Actions workflows.




