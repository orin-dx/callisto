# callisto — polyglot version coordination for Cargo, npm, and PyPI monorepos

**Status:** IMPLEMENTED & VERIFIED (Canonical Spec)
**Date:** 2026-07-24
**Repo:** `github.com/orin-dx/callisto`
**License target:** AGPL-3.0 (coordination logic) + MIT/Apache-2.0 (format + model primitives) — see §16
**npm scope:** `@orin-dx`

This revision supersedes the prior draft in three structural ways, each explained in the
relevant section below:

1. **Callisto manages both versioning and publishing.** It decides what version each package
   should be next, updates the workspace to reflect that, and runs `cargo publish`,
   `npm publish`, and `twine upload` via `callisto publish` (shelling out to each
   ecosystem's own tool — never reimplementing registry HTTP protocols). See §9.
2. **Scope is deliberately narrower than "polyglot for every language."** Rust and npm are
   the committed core. Python, Go, and JVM support is real but demand-gated, not scheduled.
   Java/Scala/C are explicitly out of scope. See §2 and §17.
3. **Resolved: Option C — library-first internals, moon as the single reference
   integration**, in the narrow, four-CI-rule sense (not a symmetric multi-consumer API).
   See §0.1 and `docs/02-library-vs-moon-decision.md` for the full argument and the concrete
   §15 crate-boundary changes it produced.

---

## 0. Summary

Callisto solves a problem every mature polyglot monorepo runs into eventually: you have
Rust crates, npm packages, and (increasingly) napi-bridged packages between them, and no
single existing tool understands version coordination across all three cleanly.

- `@changesets/cli` is JS-only. Polyglot users work around this today by creating "proxy"
  `package.json` files for their Rust/Python packages so changesets treats them as
  versionable, then hand-writing a sync script that copies the bumped version out of
  `package.json` into `Cargo.toml`/`pyproject.toml` after the fact. This is a documented,
  real, current pattern (see the research brief) — not a hypothetical.
- `release-please` is genuinely polyglot (Rust, Python, Go, Java, PHP, Ruby, Node) via a
  manifest + plugin architecture, but it's Conventional-Commits-based, not changesets-based,
  and has no concept of napi platform-package coordination.
- `knope` handles Cargo + npm dual-published packages but writes exact versions everywhere
  (no npm range preservation) and has no napi awareness either.
- Nothing handles napi's "one crate, N platform packages, one main package with
  `optionalDependencies` on all of them, all sharing a version" pattern natively.

Callisto's answer: byte-compatible with the `@changesets/cli` file format (so adoption and
rollback are both one commit), with a package model that treats "one version across
multiple manifests" as the base case (borrowed from `knope`) and extends it with a
manifest-*role* axis (canonical / platform / lockfile) that makes the napi pattern a native
case instead of a workaround.

### 0.1 Resolved: Option C, narrowly defined

Everything in this document was, until this revision, designed with an implicit assumption:
callisto is a moon extension first, with a standalone CLI as a secondary surface. Research
into `release-please`'s actual architecture (see `docs/01-research-brief.md`) complicated
that assumption in a useful way: release-please ships as a **library** (the `release-please`
npm package exports a `Manifest` class and the releaser/plugin machinery directly) with
both its CLI and its GitHub Action built as thin consumers of that library. Neither the CLI
nor the Action contains coordination logic; both just call the library and wire up I/O.

That raised a real design fork for callisto — moon-first (Option A: core crates designed
around moon's project graph as a first-class input, standalone CLI a degraded fallback),
library-first (Option B: zero-moon-dependency core exposing a stable public Rust API / C
ABI, with moon, the Action, and the CLI as three symmetric consumers), or hybrid (Option C:
library-first internals, moon as the reference integration shipped and supported first).

**`docs/02-library-vs-moon-decision.md` resolved this**, against release-please, Nx
Release's v21 versioning rewrite, semantic-release, and knope as prior art, all independently
fact-checked and adversarially red-teamed before landing here. The verdict is **Option C**,
but narrower than "moon, the Action, and the CLI are three symmetric consumers" — that
premise is false in callisto's own design, since §12.1 already makes the Action a CLI
consumer (shells out to the `callisto` binary), not a library consumer. The real minimal
version of library-first is four CI-enforceable rules, not an API-design project:

1. **Zero moon in the dependency tree of the coordination core** (`callisto-format`,
   `callisto-model`, `callisto-graph`, `callisto-manifests`, `callisto-conventional`,
   `callisto-changelog`) — enforced in CI, not by convention.
2. **The core builds for `wasm32-wasip1` *and* passes its fixture suite under `wasmtime`
   with only the workspace root preopened** — a CI job from v0.1, before `callisto-moon`
   exists, because build-only conformance misses the runtime-only failures WASI's real
   filesystem API otherwise hides (compilation succeeding is not the same as working under
   moon's preopened-directory sandbox).
3. **Compute/apply split**, borrowed from release-please's `Manifest.buildPullRequests()`/
   `createPullRequests()`: every command is a pure function returning a plan value, plus a
   separate step that touches disk.
4. **The stable public contract is `--format json` on stdout** (§12.5), not a semver-stable
   Rust API — the Rust API of AGPL crates stays explicitly unstable pre-1.0.

Concretely for §15: `MoonProjectGraphResolver` is **deleted** — moon's project-graph edges
carry no version-requirement string (verified against moon's actual `ProjectDependencyConfig`
source), so it structurally cannot supply what §7.4's cascade needs. In its place, a
narrower `ProjectLocator` trait splits moon's real value into two independent questions:
project *discovery* (moon is authoritative when present — `MoonProjectLocator` supersedes
the `ignore`-crate walk outright) and dependency *edges* (moon is a non-authoritative
cross-check only, via an optional `declared_edges()` returning callisto-owned
`DeclaredEdge`/`DeclaredEdgeKind` values in `callisto-model`, never moon's own
`DependencyScope` type — see §15 for the exact shape and §7.5 for the identical pattern
applied to napi's `napi.targets` cross-check). `DependencyResolver` keeps its second
implementor by being reused for `callisto-fixtures`' in-memory test graphs, not for moon.
Plan/report value types move into `callisto-model` (MIT/Apache tier, §16) since they are the
public contract per P7, not `callisto-graph`'s AGPL tier. See §15 for the full crate layout
and the decision doc for the complete argument, including why the licensing posture (§16)
was initially overweighted as a reason for Option C and was cut back to a supporting, not
load-bearing, point.

---

## 1. Naming

**callisto** — Jupiter's second-largest moon, the most heavily-cratered body in the solar
system. The metaphor: Callisto's surface has never been geologically resurfaced, so every
impact over ~4 billion years is still visible, layered, in order — which is what a
versioning tool preserves for a codebase.

Confirmed clean on crates.io (no `callisto` or `callisto-*` occupation). npm bare name is
held by an abandoned framework (9 years stale); we publish scoped as `@orin-dx/callisto`.
Adjacent-but-unrelated namespace noise exists (a cryptocurrency project, a video game) —
neither blocks us, both are worth a one-line disambiguation in the README.

Repo: `github.com/orin-dx/callisto`. Action: `github.com/orin-dx/callisto-action`.

---

## 2. Scope: what callisto is and isn't for

### 2.1 The polyglot cases, enumerated

| Case | Description | Existing tool coverage |
|---|---|---|
| A | Pure Cargo workspace | `cargo-release`, `knope` |
| B | Pure npm workspace | `@changesets/cli`, `knope` |
| C | Cargo + npm workspaces, disjoint (no cross-refs) | Poor — two disconnected flows |
| D | Dual-published single package (one source, `Cargo.toml` + `package.json`, same version) | `knope`'s `versioned_files` |
| E | napi single package (1 crate → N platform npm packages + 1 main package with `optionalDependencies`, shared version) | **Nothing** |
| F | napi package inside a larger polyglot monorepo, cross-registry deps everywhere | **Nothing** |

Cases E and F are the wedge — the reason callisto exists rather than "just use
release-please."

### 2.2 Ecosystem priority — committed vs demand-gated

An earlier draft of this design treated "support every language" as an implicit goal and
grew scope accordingly (Python, Go, Java, Scala, C#, Ruby, Deno all got design attention).
An adversarial review of that draft concluded the scope had outrun the addressable market:
callisto's actual niche is narrow (changesets-format preference, napi coordination), and
building for languages nobody in that niche has asked for is waste.

Revised posture:

| Ecosystem | Status |
|---|---|
| Rust / Cargo | **Committed, v0.1** |
| npm (npm/pnpm/yarn workspaces) | **Committed, v0.1** |
| napi platform packages | **Committed, v0.3** |
| Python (uv/hatch/poetry, PEP 440) | Real, tractable, **demand-gated** — not scheduled until a user asks. Design supports it (§7.7) but it is not release-blocking for any v0.1–v0.4 milestone. |
| Go modules | Real, tractable (git-tag-only publish, no registry backend needed), **demand-gated**. |
| Maven / Gradle-catalog / sbt-version-file | Tractable under the versioning-coordinator posture (§9) because callisto never touches Maven Central/Sonatype's publish machinery — only version bumps and dependency-graph coordination. **Demand-gated.** |
| NuGet, Deno/JSR, Ruby | Tractable, **demand-gated**. |
| Gradle/sbt imperative DSL write-back (`build.gradle`, `build.sbt`) | Read-only if ever built; write-back requires AST-aware editing of Turing-complete build scripts and is **not planned**. |
| Java/Scala versioning grammar in the abstract | Supported by the trait design (see §7.7) whenever an ecosystem needs it — Maven's version-comparison rules are well-specified and portable. |
| C / vcpkg / Conan | **Not building.** C libraries version through distro packaging, not per-package registry publishes. The polyglot-monorepo-with-coordinated-releases shape callisto addresses doesn't exist for C. |

The trait boundaries in §7 and §15 are designed so that adding any demand-gated ecosystem
is bounded work (one grammar impl, one manifest impl, one identity-resolution note) — not
because we're building toward all of them, but because the abstraction should not make any
of them artificially hard if a real user shows up needing one.

**Where this scope discipline actually lives, concretely**, since it's worth being honest
about the difference: §5.1/§5.2's `Ecosystem`/`PublishTarget`/`ManifestFormat` enums *do*
fully enumerate the demand-gated ecosystems (`Pypi`, `Go`, `Maven`, `SettingsGradle`, etc.),
and §7.7/§7.8 give real paragraphs to PEP 440 and Maven's comparator. That's not a
contradiction of the cut above — it's the trait-boundary cost P4 asks for, and it's cheap
specifically *because* it stops at declared-but-unimplemented enum variants and prose. What
the earlier draft actually did differently, and what this revision cut, was writing real
per-ecosystem behavior *design* (config surface, edge cases, migration guidance) for
languages nobody had asked for — not naming the variants a trait needs to stay open. If a
future edit starts fleshing out unimplemented variants' behavior beyond what §7.7/§7.8
already say, that's scope creep by this section's own definition and should be caught by
the same adversarial-review posture that produced this cut in the first place.

---

## 3. Prior art and positioning

### 3.1 Comparison table

| Feature | `@changesets/cli` | `release-please` | `Nx Release` | `knope` | callisto |
|---|:---:|:---:|:---:|:---:|:---:|
| Intent-capture format | Changesets (Markdown+YAML) | Conventional Commits | Either | Changesets or commits | Changesets (byte-compat) |
| Cargo workspaces | ✗ | ✓ (`cargo-workspace` plugin) | ✓ (community plugin) | ✓ | ✓ |
| npm workspaces | ✓ | ✓ (`node-workspace` plugin) | ✓ | ✓ | ✓ |
| Cross-ecosystem dep cascade | ✗ | ✓ (per-ecosystem workspace plugins) | Partial | ✗ | ✓ |
| Dual-published packages | ✗ | Partial (two configs, one path) | ✗ | ✓ | ✓ |
| napi platform-package coordination | ✗ | ✗ | ✗ | ✗ | ✓ |
| Ships as a library | Partial | **✓ (`Manifest` class exported)** | Partial | ✗ | narrow — moon-agnostic core, unstable pre-1.0 Rust API, JSON is the stable contract (§0.1) |
| Runs as moon extension | ✗ | ✗ | ✗ | ✗ | ✓ |
| Range preservation (npm) | ✓ | N/A (mostly Conventional-Commits-driven) | ✓ | ✗ | ✓ |
| Publishes packages itself | Via `changesets/action`'s `publish` script | Optionally (via separate publish step) | ✓ (built-in) | Via workflow | **✗ — deliberately** (§9) |

### 3.2 Where callisto sits

Callisto is not "the polyglot release tool." That space has real, mature, well-resourced
incumbents:

- **release-please** — Google-backed, genuinely multi-language, manifest+plugin
  architecture, ships as a library. The default choice for a team starting from scratch
  today with no changesets legacy and no moon commitment.
- **Nx Release** — deeply integrated with Nx, handles npm/Rust/Docker, actively investing
  in polyglot support (Java/Gradle, .NET announced for 2025). The default choice for teams
  already on Nx.

Callisto's defensible niche is narrower and specific: **teams that prefer the changesets
file format over Conventional Commits, and need napi-style cross-registry coordination
that nothing else handles.** Both conditions should plausibly hold for the same team to
make callisto the right choice over the incumbents. This is a real niche — changesets has
a large loyal JS user base that dislikes Conventional-Commits-only tools, napi coordination
is a documented pain point nobody solves — but it is a niche, not a mass-market claim.

The core itself doesn't require any particular build system to use: it runs as a
standalone CLI (Bazel sandboxes, Buck2, Nix flakes, GitHub Actions, GitLab CI, local Git
hooks) or as a moon WASM extension. moon is one first-class integration among several —
the reference one, in the narrow §0.1 sense — not a precondition for callisto to be
useful.

**Risk to name plainly:** the changesets team has an open, long-standing issue (#665)
requesting native polyglot support. If `@changesets/cli` ships that natively, callisto's
"byte-compatible changesets format + polyglot" positioning weakens to "moon-native
implementation of the shared format" — still real, but narrower. This hasn't happened in
several years of the issue being open, so it's a background risk, not an imminent one.

---

## 4. Design principles

**P1 — Byte-compatibility with `@changesets/cli`'s file format is a hard requirement.**
Same `.changeset/*.md` shape, same `pre.json` shape, same `bump_version` semantics
(including the "no 0.x-to-0.(x+1) remap on explicit major" behavior). One-commit adoption,
one-commit rollback. This is the adoption gate; nothing overrides it. **Scope note:** the
rollback guarantee is lossless only for `ReleaseTrigger::Changeset` packages, since that's
the only trigger with a changesets-native file to roll back to — `Auto`-trigger packages
(§5.1, §7.1) infer from conventional commits, a computation `@changesets/cli` has no
equivalent of, so there is nothing for P1 to be byte-compatible *with* on that side. This is
an honest scope boundary, not a violation of P1 — see §7.1's pre-major-inference discussion
for where the line is drawn in practice.

**P2 — Statelessness for correctness; state only as a hint.** "Does this need publishing?"
is answered by comparing on-disk state to git tags, never by trusting a file that might not
survive a branch merge or a tool-version upgrade. Any state file callisto writes is
`.gitignore`d and never load-bearing for a correctness decision.

**P3 — Idempotence per phase.** Every operation callisto performs is safe to re-run. A
partial failure followed by a retry does not double-apply anything.

**P4 — Polyglot uniformity, including grammar and workspace concept, not just manifest
format.** Cargo, npm, and (when demand-gated ecosystems are added) Python/Go/Maven are all
"an ecosystem" behind the same traits. Adding one is bounded: one grammar impl, one
manifest impl, one identity-resolution note. Anywhere the design assumes SemVer implicitly,
that's a trait boundary in disguise and should be named as one.

**P5 — Structural fixes over discipline.** When a class of failure is identified, the fix
belongs in code (a check, an invariant, a refusal to proceed) — not in a README section
telling people to remember something. See §13 for the concrete invariants this principle
has produced.

**P6 — Release semantics live in the coordination core; every wrapper is a thin
dispatcher.** (Revised from an earlier "zero logic in the wrapper" phrasing, which was too
strong — wrappers legitimately contain orchestration logic like binary installation,
platform detection, and JSON-to-GHA-output translation. What they must never contain is
*release semantics*: computing bumps, resolving cascade, deciding publish order, writing
changelogs. If a wrapper needs to compute any of those, that's a missing capability in the
core, not something to write in the wrapper.)

**P7 — Every boundary is explicit and testable.** Between the core and any integration
(moon, the Action, a future Nx plugin): a versioned, fixtured JSON/API contract. Between
callisto and upstream tools it shells out to (`git`, and whichever ecosystem tools it
queries): a runtime capability check with a clear error on mismatch, not a silent
assumption. Between callisto's own artifacts (e.g., a `setup-moon-callisto` composite
delegating to `callisto-action`): exact-pin delegation with an automated bump path, never
floating-major delegation between things we both own.

---

## 5. Package model

### 5.1 Core types

```rust
struct Package {
    id: PackageId,
    manifests: Vec<ManifestDecl>,   // ManifestDecl: §5.2 — renamed from `Manifest` to avoid
                                     // colliding with the unrelated `Manifest` trait (§15),
                                     // which is the runtime file-handle abstraction, not this
                                     // declared path/role/format triple
    changelog: Option<PathBuf>,
    release_trigger: ReleaseTrigger,
    publish_to: Vec<PublishTarget>,
    tag_template: Option<String>,
}

enum PackageId {
    Bare(String),
    Prefixed { ecosystem: Ecosystem, name: String },
}

// Referenced throughout §7 and §15's trait signatures; declared here because "Core types"
// is where an implementer looks for them, not scattered across the sections that use them.
enum Severity { Major, Minor, Patch, None }   // §6.1's changeset severities as a Rust type

enum DepKind { Runtime, Dev, Peer, Optional, Build }   // an edge's dependency kind (§7.2)

struct DepEdge {
    from: PackageId,
    to: PackageId,
    kind: DepKind,
    spec: DepSpec,   // §7.3
}

struct DependencyEntry {   // one raw dependency record as read off a single manifest,
    name: String,           // pre-resolution to PackageId (§5.4/§7.2) — distinct from
    kind: DepKind,           // DepEdge, which is post-resolution and graph-level
    spec: DepSpec,
}

enum WorkspaceKind { Pnpm, Yarn, Npm }   // §7.3's `DepSpec::Workspace` payload

enum Ecosystem {
    Cargo,
    Npm,
    // Demand-gated, declared for forward compatibility, not yet implemented:
    Pypi,
    Go,
    Maven,
    NuGet,
    Deno,
    Jsr,
}

enum ReleaseTrigger {
    Changeset,                 // only bumps when a changeset names it
    Auto,                      // infers from conventional commits if no changeset present
}

enum PublishTarget {
    CratesIo,
    Npm { registry: Option<String> },
    // demand-gated:
    Pypi { index: Option<String> },
    NuGet { source: Option<String> },
    // orphaned by §9.5's scope cut: creating a GitHub Release is a registry-publish-shaped
    // action, and §9.5 removed octocrab/reqwest/GitHub-release creation from callisto's
    // dependency tree entirely. Retained as a variant only if a calling workflow's own `gh
    // release create` step wants callisto's plan to name it as a target for planning
    // purposes; callisto itself never creates one — not demand-gated the way Pypi/NuGet are,
    // just a plan-shape label with no corresponding execution capability in callisto.
    GitHubRelease,
    None,                      // internal-only, never published
}
```

### 5.2 Manifest role — the axis `knope`'s model lacks

```rust
struct ManifestDecl {   // declared shape of one manifest file; the `Manifest` trait (§15)
    path: PathBuf,        // is constructed from one of these plus format-specific parsing
    role: ManifestRole,
    format: ManifestFormat,
}

enum ManifestRole {
    Canonical,                 // holds the version of record; ≥1 per package
    Platform {                 // napi-style variant; inherits parent's version
        platform: String,
        arch: String,
        abi: Option<String>,
    },
    Lockfile,                  // regenerated, not directly version-written
}

enum ManifestFormat {
    CargoToml,
    PackageJson,
    // demand-gated:
    PyprojectToml,
    SetupCfg,                  // read-only; write-target is pyproject.toml
    GoMod,
    PomXml,
    GradleVersionCatalog,
    SettingsGradle,
    VersionSbt,
    DenoJson,
    CargoLock,
    PackageLockJson,
    PnpmLockYaml,
}
```

**Case-to-model mapping:**
- Cases A/B (pure workspace): one `Canonical` manifest per package.
- Case D (dual-publish): one package, two `Canonical` manifests (`Cargo.toml` +
  `package.json`), same version.
- Case E (napi single package): one package, canonical manifests for the crate and the main
  JS package, N `Platform` manifests, `Lockfile` entries for `Cargo.lock` and the JS
  lockfile.
- Case F (composite): each logical package modeled independently; §7's graph handles the
  cross-package edges.

### 5.3 napi platform auto-derivation

Callisto reads `napi.targets` from the main package's `package.json` and synthesizes
`Platform` manifests + a `fixed-group` binding automatically. This is a **best-effort
suggestion, not authoritative** — `callisto init` offers to write the derived group into
`callisto.toml` as explicit config on first run, after which it's a normal user-owned group.
Steady-state operation doesn't re-derive on every version pass; this avoids permanent
coupling to `@napi-rs/cli` internals.

**Membership drift after promotion.** Once a group is user-owned, `napi.targets` can still
be read cheaply on every run as a **non-authoritative cross-check** against the group's
configured `members` — the same "config is authoritative, external signal is diagnostic
only" pattern used for moon's *edge* cross-check (§18 Q1b — not Q1a, where moon is
authoritative for discovery; napi's case is analogous only to the diagnostic half). See §7.5
for how a target
added/dropped from `napi.targets` is surfaced and reconciled; the short version is that the
cross-check never auto-mutates `callisto.toml` — `callisto init` (or a future `--sync`
variant of it) remains the only path that writes group membership, so every membership
change is a reviewable config diff, not a silent rewrite.

### 5.4 Package identity resolution

`Bare(name)` when unambiguous workspace-wide. `Prefixed { ecosystem, name }`
(`cargo/foo`, `npm/foo`) when the same string names packages in more than one ecosystem —
this is how the Case D (dual-publish) pattern lets a contributor **name** one side
explicitly in a changeset, for clarity/attribution, even though both entries resolve to the
*same* `Package` (§5.2: Case D is one package, two `Canonical` manifests, one version of
record — that model doesn't change here). Severities still aggregate by max (§7.1)
regardless of which prefix named them:

```markdown
---
"cargo/foo": patch
"npm/foo": minor
---
```

resolves to a single `minor` bump written to both `Cargo.toml` and `package.json` — **not**
two independently-versioned sides. If a team genuinely wants the crate and the npm binding
to version independently (diverging, not just merging), that's a different case entirely:
model them as two separate `Package`s and, if they should usually-but-not-always move
together, bind them with a **linked** group (§7.5) rather than Case D's single-package
identity — linked groups already have the right semantics (share a version only when
jointly touched; independent lines otherwise) for that need, and Case D's prefixed-identity
mechanism shouldn't be overloaded to also mean that.

Resolution order for a name found in a changeset: exact bare match → exact prefixed match →
implicit disambiguation via sibling entries in the same changeset → error listing
candidates. On write (`callisto add`), emit the shortest unambiguous form.

For Maven-shaped identity (`groupId:artifactId`), when that ecosystem is built: the colon
form goes into the `name` slot as-is; ecosystem-prefixed disambiguation becomes
`maven/org.example:foo-core`.

**Identity resolution for graph edges (§7.2), not just changeset entries.** Walking a
manifest yields raw dependency-name strings, which need two things this section doesn't
otherwise state: (a) discriminating a workspace-member dependency from an external
registry dependency of the same name is the graph-construction step's job, done by checking
whether the name resolves to a known `PackageId` in the workspace *before* falling back to
treating it as an opaque external dependency (never the reverse — an external package
sharing a name with a workspace member must not be misread as an internal edge); (b) for a
Case D package, an edge reaching it via either its `Cargo.toml` or its `package.json` side
must resolve to the *same* `PackageId` — the single-`Package`, aggregate-by-max model above
depends on this, since a graph that saw "cargo/foo" and "npm/foo" as different nodes would
silently reintroduce the two-sides-diverge behavior this section just ruled out.

---

## 6. Changeset file format

Byte-compatible with `@changesets/cli`, enforced by a round-trip test suite against
fixtures pulled from the reference tool's own corpus.

### 6.1 Shape

```markdown
---
"@myorg/foo": minor
"cargo/foo": patch
---

Summary paragraph. Plain text as far as callisto is concerned.
```

- `---`-delimited YAML frontmatter, each line `<name>: <severity>`.
- Parse via `rsplit_once(':')` **after** unquoting — not before. (The reference
  `knope-dev/changesets` crate gets this backwards and mangles quoted `@scope/name`
  entries; we do not depend on that crate for exactly this reason.)
- Names may be quoted or bare on read; quoted-when-necessary on write (matches
  `@changesets/cli` output).
- Severity: `major | minor | patch | none`, case-insensitive read, lowercase write. `none`
  is first-class — a documented change with no version bump, for changesets a human writes
  by hand (docs-only edits, or any explicit no-op record). This is distinct from §7.4's
  `Severity::None` cascade outcome for out-of-range dev-deps, which is an internal
  computed value used during aggregation, not a changeset file callisto ever authors —
  callisto's mutation steps (§7.6) never write a new `.changeset/*.md` file for a cascade
  decision, only rewrite dependency specs. Two things share one enum value; only the
  file-format one is ever persisted to disk as a changeset.
- `#`-comment lines tolerated on read, dropped on write.
- Empty frontmatter valid iff summary is non-empty.
- Filenames arbitrary, sorted for deterministic read order.

### 6.2 `bump_version` semantics

Matches `@changesets/cli` exactly. No exceptions:

- `0.5.2 + major → 1.0.0` (not `0.6.0` — no 0.x-remap; this is deliberate, see the
  discussion in the previous revision for why the "smoothing" some tools do is wrong)
- Prerelease/build metadata dropped on any real bump, preserved on `none`.

### 6.3 Empty-changeset validation

**New in this revision**, informed by observing a real failure class in adjacent tooling:
a changeset with a copy-pasted or stale description can bump a version and ship release
notes describing a change that isn't actually in the diff. Nothing in the naive flow
cross-checks that a changeset's named packages actually changed.

`callisto version` verifies, for each pending changeset, that at least one of its named
packages has file changes since that package's last-release tag. A changeset naming a
package with zero changes since the last tag is either stale (should be deleted) or
misnamed (wrong package) — warn by default, hard-fail with `--strict`, escape hatch via
`--allow-empty-changesets` or `[validation].allow-empty-changesets = true` for intentional
re-releases (e.g., a security-only re-tag with no code diff).

**`--strict` composition.** This spec uses `--strict` (here and for §7.5's `napi.targets`
drift cross-check) and `--strict-graph` (§7.2's moon cross-check, distinctly named) as
independent, per-check flags, not one global strictness level — `--strict` alone promotes
*this* command's own warn-by-default validations (empty-changesets, napi drift) to hard
failures; `--strict-graph` is separate because the moon cross-check is itself opt-in (not
every workspace runs under moon), so it needs to be escalatable independently of whatever
else `--strict` is doing. They compose freely (`--strict --strict-graph` hard-fails both
classes); neither implies the other.

This is a structural check (P5), not a documentation ask.

### 6.4 Pre-release state (`pre.json`)

Byte-shape-compatible with `@changesets/cli`: filename, camelCase fields (`mode`, `tag`,
`initialVersions`, `changesets`), 2-space JSON, trailing newline. See §8.

---

## 7. Version computation and cascade

### 7.1 Aggregation

Load pending changesets → validate named packages exist → aggregate per-package severity
by max. `ReleaseTrigger::Auto` packages with no changeset get inferred severity from
Conventional Commits since last tag (`<type>!:`/`BREAKING CHANGE:` uppercase-only → major,
`feat` → minor, `fix`/`perf` → patch). A changeset always wins over inference.

**Pre-major inference (`callisto-conventional`, v0.2, referenced by P1's scope note in
§4).** `bump_version` (§6.2) stays rigid — no 0.x remap, no flag, no config path reaches it,
ever. That rigidity only has teeth for `Changeset`-trigger packages, because that's the only
trigger with a real `@changesets/cli` file to be byte-compatible *with*; `Auto`-trigger
inference has no changesets-native equivalent to diverge from. So the opt-in remap this
principle would otherwise forbid lives one layer up, entirely inside inference: an opt-in
per-package/per-group `pre-major-inference = "conservative"` setting that maps `breaking →
Severity::Minor` and (separately gated) `feat → Severity::Patch`, applied only when
producing a severity for an `Auto`-trigger package whose current on-disk version is `0.y.z`
with `y > 0`. Default off. Inert (no remap, tool says so explicitly) when the current
version is `0.0.z` or there is no prior tag — that exact boundary is where release-please's
own `bumpMinorPreMajor` has leaked bugs for years (its issues #2087/#2635), so it's a tested
case from day one, not an emergent one. The existing "a changeset always wins over
inference" rule composes for free: an explicit `major` in a changeset still produces the
real major bump regardless of this setting, since the setting only ever touches inferred
severities. Document the divergence from `@changesets/cli` loudly in the v0.4 migration
guide, in the same breath as the one-commit-rollback promise.

**Fixed-group severity union — resolves the group-forcing-vs-cascade-fixpoint ordering
question.** Before §7.4's cascade runs to fixpoint, any severity assigned (by changeset or
inference) to *any* member of a fixed group (§7.5) is unioned by max across every member of
that group. This is a single pre-step, not something interleaved with or run after the
fixpoint: by the time §7.4 starts, fixed-group members already carry identical target
severities, so the fixpoint treats them uniformly and correctly cascades their dependents in
one pass — no second pass is needed to "catch up" a group member the fixpoint moved. Given
identical starting severities and (per §7.5) identical aligned base versions, the members
necessarily converge on the same resulting version without any separate version-forcing
step at write time; §7.6 step 2's "alignment re-check" is therefore a verification that this
held, not a corrective write (the one exception remains the new-member exemption in §7.5,
which is a write, not a check).

### 7.2 Dependency graph

Nodes are packages; edges are typed (`Runtime | Dev | Peer | Optional | Build`, matching
`DepKind` in §5.1) with a `DepSpec`. Built by walking every `Canonical` manifest — this is
`ManifestWalkResolver`, the sole `DependencyResolver` impl (§0.1, §15). When moon is
available, its project graph is authoritative for project *discovery* (`ProjectLocator`,
§15) and a non-authoritative *cross-check* for declared edges only (`declared_edges()`,
§15) — moon's edges carry no version-requirement string, so they can never replace this
manifest-derived graph, only diagnose disagreement with it (warn by default,
`--strict-graph` hard-fails, surfaced in `--format json`).

### 7.3 `DepSpec`

```rust
enum DepSpec {
    Exact(Version),
    Range(VersionReq, String),       // parsed + original string, for lossless round-trip
    Workspace(WorkspaceKind),        // pnpm workspace:* etc — never bumped, pnpm resolves
    Catalog(Option<String>),         // pnpm catalog reference
    CargoBare(Version),              // Cargo's bare "1.2.3" (semantically caret)
    Opaque(String),                  // anything unrecognized — left untouched
}
```

Parse once at graph construction; keep the original string. If a bump can't be confidently
round-tripped back to a matching string (complex multi-clause ranges), fall back to
`Opaque` and warn — silent range corruption is worse than a loud please-review.

### 7.4 Cascade rules

| Edge kind | Spec covers new version | Action on dependent |
|---|:---:|---|
| Runtime / Optional / Build | ✓ | none |
| Runtime / Optional / Build | ✗ | patch bump + spec rewrite |
| Peer | ✓ | none |
| Peer | ✗, patch source | patch bump + spec rewrite |
| Peer | ✗, non-patch source | **major bump** + spec rewrite (breaking peer upgrades are breaking for the dependent's own consumers) |
| Dev | ✓ | none |
| Dev | ✗ | **`Severity::None`** — spec rewrite only, no version bump |

Runs to fixpoint. Default `[cascade].mode` is `"out-of-range"` (cascade only when the spec
no longer covers); `"always"` (cascade every dependent regardless of range coverage) is
exposed in config from day one for teams that want forced downstream re-publishes. This is
a distinct axis from `[cascade].bump-severity` (§14), which controls how hard a cascaded
dependent bumps once triggered, not whether it cascades at all.

### 7.5 Fixed and linked groups

**Fixed** — members always share the exact version; validated pre-mutation (hard error if
already divergent). Used for napi main+platform coordination.

**Linked** — members share a version only when jointly releasing; independent version
lines otherwise. **"Jointly releasing" means jointly *named*, not jointly *cascaded to*** —
this is the deliberate difference from fixed groups' §7.1 severity-union pre-step, which
does apply to cascade-induced bumps. A changeset or inference assigning severity to ≥2
linked members in the same run counts as joint and unions their severity by max, exactly
like a fixed group would; a cascade-induced bump (§7.4) landing on only one linked member,
with the rest untouched, does **not** pull its linked siblings along — cascade is a
mechanical consequence of a dependency edge, not an expression of release intent, and
"jointly releasing" is about intent. A linked member that diverges via cascade this way
simply starts (or continues) an independent version line until a future changeset/inference
event touches ≥2 members again.

**Platform manifests are never independently tagged.** Only the napi main package (and the
crate, if it's separately published to crates.io) represent a real release point in
`plan-publish`'s `releases[]` (§9.2) and get a git tag; the N platform packages are
dependents-in-lockstep, not independently released artifacts, so there is nothing to tag
per-platform and no orphan-tag cleanup to worry about when a platform is later dropped.

**Handling `napi.targets` drift (added/dropped platform targets).** A fixed group's
`members` list is config, and config is authoritative for what a version bump actually
touches — `napi.targets` is read every run purely as a cross-check (§5.3), never as a
membership source:

- **Target added to `napi.targets` but not yet in `members`** — warn by default (`--strict`
  hard-fails), same shape as §6.3's empty-changeset validation. Never auto-added to
  `callisto.toml`; the user accepts the diff via `callisto init`'s existing "offer to write
  the derived group" flow, which then shows up as a normal, reviewable config change.
- **`members` lists a platform no longer in `napi.targets`, but its manifest file is still
  present on disk** — same warn/`--strict` cross-check, prompting the human to either delete
  the leftover package directory or accept the config update removing it.
- **`members` lists a platform whose manifest file is missing entirely** — this is not a
  drift diagnostic, it's a hard error unconditionally: a config-declared required member
  that doesn't exist on disk fails loudly (P5) rather than being silently skipped.
- **A member actually removed from both `members` and disk** — no special handling needed;
  it simply stops participating in future bumps. Nothing to clean up, per the no-independent-
  tagging point above.

**New members are exempt from the pre-mutation divergence check.** A member with no prior
release tag (i.e., genuinely joining the group — freshly scaffolded by `@napi-rs/cli` at
whatever placeholder version it was given, e.g. `0.0.0`) is not "divergent" in the sense
§7.6 step 2 checks for; divergence is only a meaningful concept between two versions with
release history. Such a member is exempted from that check and is unconditionally force-set
to the group's target version the first time §7.6 step 4 writes platform manifests — no
separate initialization logic is needed, since inherit-parent-version already overwrites
whatever placeholder was there. After that first inclusion, the member is subject to the
normal alignment check on every subsequent run like any other group member.

### 7.6 Mutation phase ordering

1. Rerun-safety check: re-read on-disk version, compare to the value captured at
   aggregation time, hard-error on drift.
2. Fixed-group alignment re-check — members with no prior release tag are exempt from the
   divergence check (§7.5); the `napi.targets` membership cross-check (§7.5) also runs here,
   warn-by-default / `--strict` hard-fail, never mutating config.
3. Write canonical manifests (format-preserving: `toml_edit` for TOML, custom
   formatter-preserving editor for `package.json`).
4. Write platform manifests (inherit parent version).
5. Update `optionalDependencies` in napi main packages to new exact platform versions.
6. Rewrite dependency specs per cascade decisions, preserving range operators.
7. Prepend changelog entries.
8. Delete consumed changeset files — only after all prior writes succeed.
9. Optionally regenerate lockfiles (`--refresh-lockfiles`, off by default; can be slow at
   scale, often better as its own moon task).
10. Optionally write `.callisto/plan.json` — a machine-readable summary, `.gitignore`d,
    never load-bearing (P2). `compose-pr-body` does **not** read this file — it runs before
    `version` (§12.2, §13 invariant 23) and reads the changeset files directly, before step
    8 deletes them; `.callisto/plan.json` is purely a post-mutation summary for other
    tooling, per §9.1.
11. Stage modified files to git. Never commit.

### 7.7 Versioning grammar is per-ecosystem, not implicitly SemVer

Every place this design currently reads as SemVer-specific is a trait boundary: `bump_version`
is a method on a per-ecosystem `Versioning` trait, not a free function. SemVer covers Cargo
and npm. Whenever a demand-gated ecosystem is actually built:

- **PEP 440** (Python) covers `X.Y.Z` releases and `aN`/`bN`/`rcN` pre-releases with
  drop-on-bump semantics matching SemVer's. Post-releases, dev-releases, and epochs are
  explicitly out of scope until someone needs them — callisto errors clearly rather than
  guessing at semantics nobody has specified.
- **Maven-style** (Java/Scala/sbt-published) versions use the Maven comparator (qualifier
  ordering like `1.0-alpha < 1.0 < 1.0-SNAPSHOT`), a well-specified ~300-line algorithm.
- **Go** has no version field to write in `go.mod` — the git tag *is* the version. Stateless
  detection (§9.1) degrades to signal-based (pending changeset or commits-since-tag) rather
  than manifest-vs-tag comparison, because there's no manifest value to compare. This is a
  real deviation from the "compare on-disk to tag" uniformity claim and should be documented
  as such whenever Go support ships, not glossed over.

### 7.8 Ecosystem write-target conventions

Where callisto writes when an ecosystem has more than one manifest convention:

- Python: `pyproject.toml` preferred; hard error (not silent skip) if only `setup.py`
  exists, directing the user to migrate.
- JVM: `pom.xml` fully writable (well-structured XML); Gradle's `libs.versions.toml`
  version catalog and `settings.gradle[.kts]` are writable; imperative `build.gradle`/
  `build.gradle.kts`/`build.sbt` are not (would require AST-aware edits into Turing-complete
  scripts) — same "migrate to the supported convention" posture as Python's setup.py case.
- Go: `go.mod` for downstream `require` line updates; no write for the module's own version
  (it doesn't have one — see §7.7).

---

## 8. Pre-release mode

Byte-compatible with `@changesets/cli`'s `pre.json` (§6.4).

`callisto pre enter <tag>` snapshots every package's current version into
`initialVersions`, sets `mode: "pre"`. Subsequent `callisto version` runs bump from
`initialVersions` (not on-disk), keeping `pre.0 → pre.1 → pre.2` monotonic, and only
consume changesets not already recorded in `pre.changesets` — so repeated runs don't
re-increment untouched packages.

`callisto pre exit` flips `mode: "exit"` without deleting the file. The next `version` run
compounds the full accumulated set into a real, non-prerelease version, then deletes
`pre.json`.

**`initialVersions` is a bump-computation input only, never an alignment-check input.**
§7.6 step 2's fixed-group alignment check (and §7.5's drift cross-check) always compares
**on-disk** versions, in pre-mode exactly as in normal mode — `initialVersions` feeds the
bump math (§8's `pre.0 → pre.1` counter), it does not replace on-disk state as the source of
truth for "are these members currently aligned." This is a deliberate, single answer to
what was previously an unaddressed ambiguity.

**A napi target added to `napi.targets` mid-pre-cycle** is handled by composing two
already-specified rules rather than adding a new one: §7.5's new-member exemption
(force-set to the group's current target version, no divergence error) applies exactly as
in normal mode, and at that same moment an `initialVersions` entry is synthesized for the
new member equal to the group's *current* `initialVersions` entry — giving it a proper
pre-release baseline retroactively rather than leaving it absent from `pre.json`. §7.5's
drift cross-check firing every run for the duration of the pre-release cycle until resolved
is expected behavior, consistent with how every other warn-by-default cross-check in this
spec works — no pre-mode-specific suppression.

**`ReleaseTrigger::Auto` packages in pre-mode** cannot be tracked via `pre.changesets`
(byte-compatible with `@changesets/cli`, which has no conventional-commits concept, so
`pre.json`'s schema has no field for "commits already counted" — and P1 forbids adding one).
This needs a marker distinct from §9.1's release tags — a release tag means "this got
published" (P2's stateless signal), which `version` never creates (§9.1, §13 invariant 24,
no exception). A pre-mode commit-boundary marker means something else entirely ("inference
already counted commits up to here"), so it's a **separate git ref namespace**, not a tag:
`refs/callisto/pre-cursor/<PackageId>` — written by `version` itself at the moment it
computes a pre-mode bump for an `Auto`-trigger package, alongside the manifest write (same
mutation phase, §7.6), and included in the calling workflow's existing `git push --tags`
step (§9.3) by pushing `refs/callisto/pre-cursor/*` alongside real tags — no new push step,
just a wider ref-spec on the one that already exists. Because it lives outside `refs/tags/`,
it can never collide with or be mistaken for a real release tag, so it doesn't need §9.1's
"only after success" rule at all — it's bookkeeping about *what inference already saw*, not
a signal about *what got published*. The next run's commit-based inference for that package
scans commits since this cursor ref, not since its last *stable* release tag — so
already-counted commits are naturally excluded without any new
`pre.json` field, and repeated `version` invocations in pre-mode are correctly idempotent
for `Auto`-trigger packages, the same way they already are for `Changeset`-trigger ones.

`callisto snapshot --tag <tag>` computes a transient, non-persistent version (e.g.
`0.0.0-snapshot-<sha>`) and writes it to manifests **uncommitted, untagged** — it never
creates a tag and never publishes (§9's coordinator posture applies here identically; an
earlier draft described `snapshot` as a "publish path," which was a leftover from the
pre-§9 orchestrator design and has been corrected). `snapshot`'s `--format json` output
carries the computed version and affected packages, not a `published`/`publishedPackages`
pair — actually publishing the snapshot manifests is the calling workflow's job, exactly
like every other publish in this design (§9.3's pattern). `snapshot` and pre-mode
(`pre.json` present with `mode: "pre"`) are mutually exclusive — invoking `snapshot` while
in pre-mode is a hard error, since both compute a version by a different, incompatible rule
from the same on-disk state.

---

## 9. Publish planning

**This section is the biggest change from the prior revision.** Callisto is a
**versioning coordinator**, not a release orchestrator. It decides what should happen and
updates the workspace to reflect the decision; it does not run the actual publish. The
underlying ecosystem tools (`cargo`, `npm`/`pnpm`, and whatever future ecosystem tool)
already do registry auth, retries, rate-limit handling, and idempotence — reimplementing
any of that inside callisto would be redundant and would put callisto in the business of
tracking every registry's quirks, which is not where its value is.

### 9.1 What callisto produces

Three outputs, all read-only with respect to any registry:

1. **On-disk state** — the result of `callisto version`: updated manifests, deleted
   consumed changesets, regenerated changelogs, optionally refreshed lockfiles.
2. **A publish plan** (`callisto plan-publish --format json`) — describes what needs
   publishing, to what registry, in what order, without executing anything.
3. **Git tags** — **never created at `version` time.** `version` only computes and writes
   manifests; it does not touch tags at all. Tag *names* are computed by callisto (one
   function, below) and appear in the plan's `releases[].tagName` (§9.2), but the tags
   themselves are only ever created by explicitly invoking `callisto tag` (or the workflow's
   own `git tag` using the plan's `tagName`/`sha` fields, per §9.3's example) **after the
   corresponding publish has actually succeeded** — never preemptively. This is the resolved
   answer to what was previously three disagreeing accounts of tag ownership: a tag's mere
   existence is P2's stateless signal that a release happened, so creating one before the
   publish it represents has actually succeeded would let a partial failure masquerade as a
   completed release on the next run — precisely the failure mode P2 exists to prevent, not
   cause. Pushing stays a separate, later step, always done by the calling workflow, never
   by callisto (§13 invariant 16's push-last rule) — `callisto tag` creates local tags only.

   **Tag name resolution is one function, used identically everywhere a tag name is
   needed** (§7.5's group-forcing, `releases[].tagName`, and `last_tag_for`'s search) — this
   is the concrete fix for the failure release-please's `#2207` documents (two independently-
   evolved identity/tag-resolution code paths silently disagreeing). `PackageId` +
   `tag_template` (§5.1) → tag string lives in `callisto-model`. **Grammar**: `tag_template`
   supports exactly one placeholder, `{version}`; there is no `{name}` placeholder, because
   the package name is already fixed per-`Package` (it's not something a template needs to
   interpolate — a template is scoped to one package by construction). The **default**, when
   `tag_template` is unset, is `{name}@{version}` with `{name}` substituted once from the
   `Package`'s bare or shortest-unambiguous identity form (§5.4) — this is where §12.6's
   `<name>@<version>` convention comes from; it's the default template's literal shape, not a
   second placeholder grammar. Because an arbitrary `{version}`-interpolated template is not
   generally invertible, `last_tag_for` never parses tag strings against the template — it
   validates at config-load time that a `tag_template` contains `{version}` exactly once,
   derives a literal glob from everything else in the template (`foo@{version}` → match
   `foo@*`), lists matching tags via that glob, and picks the highest by parsing only the
   substring at the placeholder's position as a SemVer (or per-ecosystem, §7.7) version —
   never by attempting to invert the template as a whole.

### 9.2 The plan's shape

```json
{
  "schemaVersion": 1,
  "rustCrates": [
    { "name": "foo", "version": "1.3.0", "publishTo": "cratesIo" }
  ],
  "npmPlatformPackages": [
    { "name": "@myorg/foo-linux-x64-gnu", "version": "1.3.0", "publishTo": "npm" }
  ],
  "npmMainPackages": [
    { "name": "@myorg/foo", "version": "1.3.0", "publishTo": "npm",
      "dependsOnPlatforms": ["@myorg/foo-linux-x64-gnu", "..."] }
  ],
  "releases": [
    { "tagName": "foo@1.3.0", "sha": "...", "changelogSection": "..." }
  ]
}
```

The **ordering implied by the plan's structure** (Rust crates → platform npm packages →
main npm packages) is the correctness-relevant fact callisto is responsible for — napi main
packages reference their platforms via `optionalDependencies` at exact versions, so those
must exist on the registry first, or installs `404`. Getting this ordering right is
callisto's job; *executing* the publishes in that order is the calling workflow's job.
`rustCrates[]`'s array order **is** the intra-release topological order (§13 invariant 7) —
load-bearing, not incidental, and part of the fixtured contract (§12.5/§12.6).

`releases[].sha` is **HEAD at `plan-publish` invocation time** — callisto never commits
(§7.6 step 11), so this is necessarily after the calling workflow's own commit of
`version`'s output (§9.3's example runs `git commit` before `plan-publish`), never before.
A workflow that reorders those two steps gets a `sha` for the pre-bump commit, which is a
misuse of the contract, not an ambiguity in it.

`releases[]` contains one entry per **tag-bearing** package only — the napi main package and
(if separately published) the crate. Platform packages appear in `npmPlatformPackages[]` for
publishing but never in `releases[]`: they are never independently tagged (§7.5), since they
are dependents-in-lockstep with the main package's release, not separate release points.

### 9.3 What this looks like in practice

```bash
callisto version
git commit -am "chore: version packages"

callisto plan-publish --format json > plan.json

# Rust: topo-sorted from the plan
jq -r '.rustCrates[].name' plan.json | while read c; do cargo publish -p "$c"; done

# npm: platforms first, mains second — the plan's structure already encodes this order
pnpm -r publish --filter "$(jq -r '.npmPlatformPackages[].name' plan.json | paste -sd, -)"
pnpm -r publish --filter "$(jq -r '.npmMainPackages[].name' plan.json | paste -sd, -)"

# Tag on success — this is what closes the stateless-detection loop for next time
jq -r '.releases[] | "\(.tagName) \(.sha)"' plan.json | while read tag sha; do
  git tag "$tag" "$sha"
done
git push --tags
```

The tag creation on success is the signal that makes next run's stateless detection work —
no callisto command "reads back" what got published; the tag existing (or not) at the
expected version is the only source of truth.

### 9.4 Idempotence — two layers

**Publish idempotence** is provided by the ecosystem tools: `cargo publish` refuses to
overwrite an existing version; so does `npm publish`. Callisto relies on this for publish
retry safety rather than re-implementing "is this already published?" registry queries. If a
`plan-publish` output names a package that's already on the registry, the ecosystem tool's
own refusal is the safety net — cheap, correct, and not callisto's problem to duplicate.

**Version-apply idempotence** is provided by callisto itself: `apply_version_plan` reads each
manifest's current on-disk version before writing. If it matches `bump.to` (the target version
was already written by a prior crashed run), the write is skipped and the path is staged
without modification — safe to retry. If it matches neither `bump.from` nor `bump.to`, the
function returns `Err(GraphError::UnexpectedManifestVersion)` to require human intervention
rather than silently writing an unplanned version. Changeset paths are always staged regardless
of whether the changeset file exists on disk, so `git rm --cached --ignore-unmatch` cleans
the index on retry even after a prior run deleted the file.

### 9.5 What this removes from the prior design

The prior revision specified a 7-phase *orchestration* pipeline that callisto itself would
execute: `cargo publish -p <name> --registry <key>` calls with retry-with-backoff on
rate-limit errors, registry-API idempotence queries (`GET /api/v1/crates/...`, `npm view
...`), an `on-failure: aggregate | abort` policy, and octocrab/reqwest/tokio in the CLI's
core dependency tree for GitHub release creation. All of that is removed. It was solving a
problem — coordinating a polyglot publish sequence — that's better solved by computing the
right plan and letting each ecosystem's own, more mature tool execute its slice.

This significantly shrinks the CLI's dependency surface and the amount of registry-specific
logic that needs testing.

---

## 10. Moon integration

Per §0.1's resolution (Option C), moon is the one blessed, first-shipped integration — **not**
a symmetric second API surface (§0.1, §15). `callisto-moon` and `callisto-cli` are
structurally identical *as crates* (both thin consumers of the moon-agnostic core across the
seams enumerated in §15: `Manifest`, `ProjectLocator`, `DependencyResolver`, `CommandRunner`)
without being equally weighted *as a design commitment* — `callisto-moon`'s trait
implementations (`MoonProjectLocator` above all) get no independent API-stability promise
and are expected to break pre-1.0 the same way Nx's `VersionActions` did.

Extension APIs implemented: `register_extension`, `define_extension_config`,
`execute_extension` (dispatches subcommands), `initialize_extension` (for `callisto init`
prompts). Deliberately not implemented: `extend_project_graph` (callisto reads moon's graph,
never injects synthetic edges into it — moon's model of the workspace stays authoritative);
`extend_task_command`/`extend_task_script` (callisto doesn't wrap tasks); `sync_project`/
`sync_workspace` (deferred — a `validate`-on-sync hook is plausible but noisy by default).

Host functions used: `exec_command` (for `moon project-graph --json`, `git` calls — the
moon-side implementation of `CommandRunner`, §15), `host_log`,
`from_virtual_path`/`to_virtual_path` (the path-resolution seam, §15). No `send_request`:
an earlier draft anticipated HTTP calls for GitHub-release creation; §9.5 removed that
capability from callisto's scope entirely, and nothing else in this design makes an HTTP
request, so there is no host-side HTTP need left to name.

---

## 11. CLI vs WASM surface

Extism/WASI can't `sh -c`, so not every command works in both surfaces:

| Command | CLI | WASM |
|---|:---:|:---:|
| `add`, `status`, `version`, `pre`, `validate`, `snapshot` | ✓ | ✓ |
| `init` | ✓ | ✓ — decided explicitly (`02-library-vs-moon-decision.md` flags interactive
  `init` prompts as a candidate for CLI-only glue; resolved here as WASM-compatible since
  `initialize_extension` (§10) already gives moon's own `init` flow a host-side prompt
  surface, so no CLI-only affordance is actually needed) |
| `plan-publish` | ✓ | ✓ (read-only, no shell needed) |
| `compose-pr-body` | ✓ | ✓ |
| `tag` (creates local tags **only**, never pushes — §9.1) | ✓ | ✓ — needs `git`, available
  via `exec_command` (§10), not `sh -c`, so no CLI-only blocker |
| `publish` (re-runs plan-publish logic, then shells out to `cargo publish`, `npm publish`, or `twine` in the correct order — requires authenticated ecosystem tools present on PATH) | ✓ | ✗ |
| `completions` | ✓ | ✗ |

Since `plan-publish` is read-only data computation (no shelling out to `cargo`/`npm`), it
works fine in WASM. The reason is narrower than "no shell-out": `git`-dependent primitives
(`last_tag_for`, conventional-commit history for `Auto`-trigger inference) still need an
exec seam, satisfied by `exec_command` (§10) calling the `git` binary host-side, not by
`plan-publish` avoiding process execution altogether — this is a direct benefit of the
versioning-coordinator reframe in §9 (no `cargo`/`npm` execution needed), not evidence that
callisto never shells out to anything in WASM. `CommandRunner` (git) and the path-resolution
seam (`from_virtual_path`/`to_virtual_path`, already in §10) are the two core I/O seams
alongside `ProjectLocator` (§15) — all three are enumerated seams, not implicit.

WASM build: `cargo build --release --target wasm32-wasip1 --no-default-features --features
"wasm,cargo,npm"`. No `octocrab`/`reqwest`/`tokio` in this feature set at all now (§9.5).

### 11.1 `callisto publish`

`callisto publish` is the execution step paired with `plan-publish` (the read-only preview).
It re-runs the same plan-publish computation, then delegates to each ecosystem's own CLI in
the correctness-required order:

1. **Rust crates** — `cargo publish -p <name>` in topological order (the `rustCrates[]`
   array order from §9.2, which is the load-bearing topo-sort per §13 invariant 7).
2. **npm platform packages** — `npm publish` (or `pnpm publish`) for each platform package,
   so they exist on the registry before the main package references them via
   `optionalDependencies` (§9.2, §13 invariant 8).
3. **npm main packages** — `npm publish` (or `pnpm publish`) for each main package.

**Flags**

- `--dry-run` — prints the computed plan only; no publish commands are executed. Equivalent
  to running `plan-publish` directly, and safe to run at any time.
- `--format json|text` — controls output format. `json` emits a machine-readable record of
  each invocation's outcome (command, exit code, stdout/stderr); `text` is the human-readable
  default.

**Relationship to `plan-publish`**

`plan-publish` is the read-only preview; `publish` is the execution step. `publish` always
recomputes the plan from scratch — there is no `--plan FILE` input. Feeding a stale or
manually-edited plan file into a publish step would be a correctness hazard (P2/P3), so
`publish` holds the same stateless guarantee as every other callisto command.

**The coordinator pattern**

`callisto publish` runs publish commands but delegates entirely to each ecosystem's own CLI
(`cargo`, `npm`/`pnpm`, `twine`). It never speaks HTTP to a registry directly. Its
responsibilities are sequence enforcement, invocation, and outcome classification (success,
already-published refusal per §9.4, or unexpected failure). Retry logic, auth, and
rate-limit handling stay with the ecosystem tool.

This is the narrow scope that §9.5 preserved when it removed the broader orchestration
pipeline: §9.5 cut callisto's own retry-with-backoff, registry-API idempotence queries
(`GET /api/v1/crates/...`, `npm view ...`), and octocrab/reqwest/tokio. `callisto publish`
keeps only the coordinator role — everything that requires speaking HTTP to a registry
remains the ecosystem tool's concern, not callisto's.

**When to use `publish` vs. the §9.3 workflow pattern**

`callisto publish` is a convenience wrapper that consolidates the manual `jq | while read`
pipeline in §9.3 into one command. The §9.3 pattern remains valid and gives calling workflows
more control over each publish step (separate CI jobs, per-ecosystem retries, custom auth
setup per step). `callisto publish` is appropriate when simplicity is preferred and auth for
all ecosystems is available in the same environment.

---

## 12. The GitHub Action

### 12.1 Shape: one composite action, not TypeScript

Both the install and orchestration logic are composite (bash-in-YAML), not TypeScript. This
was not the original plan — an earlier draft of this design proposed TypeScript for the
orchestration wrapper on testability grounds. Reconsidered: the actual logic is `gh` CLI
calls, `callisto` CLI calls, and branching on JSON — TypeScript would just be `execa` calls
wrapped in a build step (`ncc`, a committed `dist/`), adding supply-chain surface for
negative benefit. A security-relevant action (download → verify → mutate → push) is more
auditable as inspectable bash than as bundled JS. This mirrors what mature internal tooling
in this exact problem space has converged on independently.

One repo, one action: `orin-dx/callisto-action`. No separate "setup" action — install-only
mode is just this action with an empty `publish`/orchestration-triggering input, matching
the shape of `component: wasm | binary | both`.

### 12.2 Modes, dispatched from inputs + `callisto status --format json`

Four+ branches:

1. **`validate-on-pr: true`** fires first, unconditionally, on PR events — `callisto
   validate --since $GITHUB_BASE_REF`, fails the check on errors. Not combined with other
   modes; this is a pure gate.
2. **`snapshot: <tag>` set** — `callisto snapshot --tag <tag>` computes and writes a
   transient version (§8), no PR, no persistent state, no tag; the action then runs the
   `publish:` input's shell command exactly as in branch 4, but receiving `snapshot`'s own
   `.version`/`.packages[]` shape (§12.5) on stdin or as a file path, **not** a
   `plan-publish`-shaped plan — `plan-publish` is never invoked in this branch, since there
   is no persisted release to plan around. The `publish:` command is the same *kind* of
   thing (a shell command the calling workflow supplies), but its input schema differs by
   branch; a `publish:` script that assumes plan-shaped JSON unconditionally needs its own
   branch-detection logic, or two distinct `publish:`/`snapshot-publish:` inputs — left as
   an implementation choice, not resolved further here.
3. **Pending changesets found** (`callisto status --format json` → `.hasChangesets`) —
   compose PR body **before** running `version`, because `version` deletes the changeset
   files `compose-pr-body` reads (§13 invariant 23 names this ordering explicitly, so it's
   not an implementation detail to rediscover on the next refactor). Then: run `version` (optionally
   `--refresh-lockfiles`), then create/update the PR, force-pushing the release branch
   *last* among the git-mutating steps in this branch.

   **PR body structure**, informed by comparing `changesets/action`'s and release-please's
   actual implementations (not just their outputs): one `<details><summary>package@version
   </summary>...</details>` collapsible section per package (release-please's pattern),
   not always-expanded `## package@version` sections (`changesets/action`'s pattern) —
   collapsed-by-default reads better for callisto's typically-larger polyglot monorepos.
   **Idempotent PR discovery** combines both tools' mechanisms rather than picking one: a
   fixed, deterministic branch name (`branch` input, changesets-style) *and* a
   callisto-owned label on the PR (release-please-style), so lookup stays correct even if
   branch-naming logic changes later; skip the update API call entirely when the composed
   body is byte-identical to the existing PR's body, to avoid audit-log churn. **Overflow**
   (body exceeds GitHub's PR-body size limit): write the full body to a file on a
   `<branch>--notes` ref and replace the PR body with a short pointer message
   (release-please's approach) rather than silently truncating changelog content
   (`changesets/action`'s approach) — for a polyglot monorepo, losing changelog content
   silently is worse than a one-hop link.
4. **No pending changesets** — call `plan-publish` and act on it per whatever the
   `publish:` input specifies (a shell command the action runs, receiving the plan on
   stdin or a file path).

### 12.3 Inputs (representative, not exhaustive)

`version` (CLI version, default latest), `component` (`wasm | binary | both`), `wasm-path`,
`repo` (override for fork-testing), `publish` (shell command; non-empty enables
orchestration mode), `cwd`, `branch`, `title`, `commit`, `setup-git-user`, `snapshot`,
`labels`/`assignees`/`milestone`, `pr-body` + rendering overrides, `dry-run`,
`validate-on-pr`.

### 12.4 Outputs (kebab-case, semantically compatible with `changesets/action`)

`resolved-version`, `wasm-path`, `binary-path` (install-mode); `has-changesets`,
`snapshot-version` (snapshot mode), `published-packages`
(`[{name, version, ecosystem, publishedTo}]`), `pull-request-number` (orchestration-mode).
No `published` boolean output — publishing is always the calling workflow's own step
(§9), so `published-packages` reflects what the *workflow* reported back via the `publish:`
command's own exit status and output, not something callisto observed directly.

**Richer than `changesets/action`'s output surface, closer to release-please's**:
`changesets/action` exposes only a flat `[{name,version}]` array, forcing any downstream
workflow step that needs to branch on one specific package's outcome to re-parse the PR body
or the plan JSON. release-please additionally exposes structured per-package outputs. Given
callisto's cross-registry polyglot case (a Rust publish job and an npm publish job in the
same workflow, wanting to know their own slice's outcome independently), `published-packages`
carries `{name, version, ecosystem, publishedTo}` so a later step can `jq`-filter it directly
rather than needing a second `plan-publish` call just to re-derive what already ran.

### 12.5 The JSON contract — the actual spec surface

This is, per the research into comparable tooling, the most important thing to get
explicit rather than implicit: **the action's entire correctness rests on the shape of
callisto's `--format json` output**, so that shape is a first-class, versioned, fixtured
contract — not an implementation detail that happens to be JSON.

Per-command shape (mandatory fields hard-gate absence; optional fields length-gate):

- `status` → `.hasChangesets` (mandatory)
- `version --refresh-lockfiles` → `.bumps[]{package,from,to,severity}` (mandatory),
  `.bumps[].governedBy` (optional — the config key responsible for a non-default-path
  decision, e.g. `"cascade.peer-escalation"`; absent when the bump followed no named
  default, §13 invariant 28), `.lockfileRefreshResults[]{filename,refreshCommand,success,
  exitCode}` (optional — absent when `--refresh-lockfiles` wasn't passed)
- `plan-publish` → the shape in §9.2; `.rustCrates`/`.npmPlatformPackages`/
  `.npmMainPackages`/`.releases` mandatory as arrays (possibly empty), absence of the key
  entirely is a hard schema failure
- `snapshot` → `.version` (mandatory), `.packages[]{name, ecosystem}` (mandatory, possibly
  empty) — no `published`/`publishedPackages` fields; snapshot computes and writes a
  transient version (§8), it does not publish, so there is nothing "published" for this
  command's JSON to report
- `compose-pr-body` → `.body` (mandatory), `.metadata.labels`,
  `.metadata.managedLabels` (optional; the managed-namespace bound is what lets the action
  safely remove stale labels it itself added without touching labels a human added by hand),
  `.metadata.overflow` (optional — `{ref, url}` when the body was written to a notes branch
  per §12.2's overflow handling, absent otherwise). **`managedLabels` semantics, confirmed
  against release-please's actual (undocumented-as-a-pattern) implementation**: it is not a
  diff against arbitrary existing labels — it is "remove exactly the labels callisto itself
  applied last time, add exactly the labels callisto wants applied now," both drawn from a
  fixed, config-known set. Human-applied labels outside that set are untouched by
  construction, not by any reconciliation logic. Expect the same edge case release-please's
  own maintainers still have an open TODO about: a label a human adds *within* callisto's
  managed set (e.g. manually applying a "snoozed" label callisto also manages) needs an
  explicit precedence rule, not an assumption that it can't happen. **Rule**: callisto's own
  last-known-applied set, round-tripped from the *previous* run's `compose-pr-body` output
  via the existing PR body (§12.5, not a new state file — consistent with P2), is
  authoritative for what gets removed; a label present on the PR but absent from that
  recorded set is treated as human-applied even if it shares a name with the current managed
  vocabulary, and is left alone. A human wanting callisto to manage a label it didn't apply
  has no supported path to do so — P5: no implicit reconciliation, not a gap.

### 12.6 The fixture harness — broader than JSON shape alone

A canary only catches what it fixtures. JSON shape drift is the obvious thing to fixture;
it is not the only contract surface that breaks downstream consumers. The fixture set
covers, at minimum:

- JSON output shape per subcommand (above)
- Tag naming conventions — the `<name>@<version>` template and how per-package
  `tag_template` interacts with it
- CLI subcommand and flag names — renaming `--refresh-lockfiles` is a breaking change and
  should fail a fixture, not just a changelog note
- Manifest write formatting — round-trip fidelity against fixture files with unusual
  existing formatting
- File paths for side effects (`.callisto/plan.json` staying exactly there)

### 12.7 Security invariants, structural

- Every user-controlled input flows through `env:`, never inline `${{ }}` interpolation in
  a `run:` block — closes template-injection, keeps the bash shellcheck-able. Verified by a
  lint step in the action's own CI that greps for the anti-pattern.
- Tokens via `GH_TOKEN`/`GITHUB_TOKEN` environment variables only, never as a plain input.
- Every downloaded artifact SHA-256 verified against its `.sha256` companion before use.
- **Composite input `default:` values must be static strings** — GitHub rejects context
  expressions (`${{ github.ref_name }}`) in `inputs.*.default` at action-*load* time, not
  at run time, so a mistake here means the action doesn't run at all, for anyone, even
  callers who pass the input explicitly. Resolve any runtime context inside the `run:`
  block via environment variables instead. Same lint step checks for this pattern too.

### 12.8 `setup-moon-callisto`

A separate, small composite that chains `moonrepo/setup-toolchain` (proto+moon) → optional
registry-auth setup → optional Rust toolchain (`dtolnay/rust-toolchain@stable` +
`Swatinem/rust-cache`) → delegates to `orin-dx/callisto-action` with `component: both`.
Explicitly does not checkout, install JS dependencies, or orchestrate a release — that's
`callisto-action`'s job. One `uses:` line for the common "moon workspace, fresh checkout, be
ready to run `moon ci`" case.

Delegates to `orin-dx/callisto-action` at an **exact pin**, never a floating major. A
scheduled CI job in the `setup-moon-callisto` repo checks weekly for newer
`callisto-action` releases and opens a bump PR. This makes staleness visible (the bump PR is
a signal) rather than silent (a floating pin would silently auto-upgrade or silently go
stale depending on direction — the asymmetry is the danger, not the coupling itself).

---

## 13. State machine invariants

Structural requirements, each traceable to a named principle in §4:

1. Frontmatter parser unquotes before splitting on `:` (not after) — P5.
2. `bump_version` matches `@changesets/cli` exactly, no remap — P1.
3. Publish-need detection is stateless (git tags), any state file is a hint — P2.
4. State files (`.callisto/plan.json`) are `.gitignore`d, never committed — P2.
5. `--format json` mode redirects all child-process stdout (`git`, `moon project-graph`,
   lockfile-refresh subprocesses — §7.6 step 9, §10's `exec_command` calls) to stderr
   structurally, at the spawn site, so nothing but the intended JSON ever reaches stdout —
   P5, P7.
6. **Callisto guarantees idempotence at two layers: version-apply and publish.**
   `apply_version_plan` is idempotent: if a manifest is already at the target version (from a
   prior crashed apply), the write is skipped and the outcome is the same as a fresh apply —
   paths are staged, changesets are removed. If the manifest is at an unexpected version,
   `UnexpectedManifestVersion` is returned rather than silently writing an unplanned bump (§9.4).
   `plan-publish`'s output is safe to consume more than once because each ecosystem tool refuses
   to overwrite an already-published version (§9.4) — the plan never asks for something that
   isn't idempotent by construction when the calling workflow does the natural thing with it — P3.
7. Cargo topological sort scoped to the intra-release set, not the whole workspace, and
   `rustCrates[]`'s array order **is** that topological order — a load-bearing contract
   fact (§9.3's worked example consumes it positionally), not just prose in §9.2, and part
   of the §12.5/§12.6 fixtured contract — P3, P7.
8. napi platform packages are ordered before their main package in the plan's structure,
   never reorderable via config — P5.
9. Peer-dep out-of-range non-patch escalates to major on the dependent by default,
   opt-out not opt-in (`[cascade].peer-escalation`, §14) — P5.
10. Dev-dep out-of-range is `Severity::None` — spec rewrite only, no version bump — P5.
11. `BREAKING CHANGE:` footer detection is case-sensitive (uppercase only), matching the
    Conventional Commits spec exactly — P5.
12. Rerun-safety check at mutation time: re-read on-disk version, compare to the value
    captured at aggregation time (§7.6 step 1) — not the plan, which is written later, at
    step 10, after mutation — hard-error on drift — P3, P5.
13. Fixed-group alignment verified before any mutation, not during — P5.
14. Schema version is a first-class, versioned contract for every `--format json` output;
    bumped atomically with the tool binary — P7.
15. Range preservation on write: if a bump can't be confidently round-tripped, leave the
    original string untouched and warn — P5.
16. **Destructive git operations (force-push, tag push) are scheduled last in any workflow
    phase.** Non-destructive operations that a re-run safely replays (local commit,
    local-only tag) may precede a fallible step; push-once operations may not, because a
    failure after a push-once step leaves state a re-run cannot cleanly repair. — P3, P7.
17. The contract surface a fixture harness must cover includes JSON shape, tag naming, CLI
    flag names, and manifest write formatting — not JSON shape alone — P7.
18. Composite-action input defaults are static strings; runtime context resolves inside
    `run:` blocks via env vars, linted for in the action's own CI — P5, P7 (§12.7).
19. Empty-changeset validation runs by default on `callisto version`, once
    `callisto-conventional`/§6.3 ship (v0.2, §17) — this invariant describes the finished
    tool's steady-state behavior, not v0.1's — P5 (§6.3).
20. Platform manifests never receive an independent git tag; only the napi main package (and
    the crate, if separately published) are tag-bearing release points — P5 (§7.5, §9.2).
21. Fixed-group `members` config is authoritative; `napi.targets` is read every run purely as
    a non-authoritative cross-check and never auto-mutates `callisto.toml` — membership
    changes are only ever written via an explicit, reviewable `init`/sync flow — P2, P5
    (§5.3, §7.5).
22. A fixed-group member with no prior release tag is exempt from the pre-mutation
    divergence check and is unconditionally force-set to the group's target version on its
    first inclusion — P3, P5 (§7.5, §7.6).
23. **`compose-pr-body` must run before `version`** in any orchestration flow, because
    `version` deletes the changeset files `compose-pr-body` reads — P5 (§12.2).
24. **Git tags are never created at `version` time**; a tag is only ever created after the
    publish it represents has actually succeeded, and only pushed by the calling workflow,
    never by callisto — creating a tag before a publish succeeds would let P2's stateless
    detection mistake a partial failure for a completed release — P2, P3 (§9.1).
25. **`PackageId` → tag-name resolution is exactly one function** in `callisto-model`, used
    identically by fixed-group forcing (§7.5), `plan-publish`'s `releases[].tagName` (§9.2),
    and `last_tag_for`'s glob-and-extract search (§9.1) — two independently-evolved
    resolution paths for the same concept is precisely the failure class documented in
    release-please's `#2207` — P5 (§9.1).
26. The coordination core (`callisto-format`, `callisto-model`, `callisto-graph`,
    `callisto-manifests`, `callisto-conventional`, `callisto-changelog`) has zero moon in
    its dependency tree, and passes its fixture suite under `wasmtime` with only the
    workspace root preopened — both CI-enforced from v0.1, before `callisto-moon` exists —
    P5, P7 (§0.1).
27. `callisto-cli/src` contains no graph-construction or cascade code — CI-enforced — the
    automated version of "don't become knope" (§0.1's decision doc) — P5, P6.
28. Every default that fires and could defensibly have gone the other way names its
    governing config key in human-readable output; the corresponding plan/report value type
    carries an optional attribution field (`.bumps[].governedBy`, §12.5) — a config key
    discoverable only by reading the docs is a design defect, not a docs gap — P5, P7
    (§18 Q5.4).

---

## 14. Config format

Hybrid: workspace-level `callisto.toml` for cross-package concerns, per-project `moon.yml`
extension block for package-scoped overrides.

```toml
# callisto.toml
[changesets]
dir = ".changeset"

[cascade]
mode = "out-of-range"   # out-of-range | always — §7.4's cascade-trigger axis: WHEN to
                         # cascade a dependent at all (default: only when its declared spec
                         # no longer covers the new version)
bump-severity = "patch"   # patch | minor — §7.4's cascade-severity axis: HOW HARD to bump a
                           # dependent once cascade fires (a distinct axis from `mode` above;
                           # earlier drafts conflated these into one four-value key)
peer-escalation = true   # peer-dep out-of-range non-patch escalates to major (§13 inv. 9);
                          # true by default, opt-out not opt-in — set false to disable
preserve-npm-ranges = true   # §7.3/§13 inv. 15 — if a bump can't be confidently
                              # round-tripped, leave the original range string untouched and
                              # warn rather than overwriting it; this key gates whether
                              # range-preserving rewrite is attempted at all (true is the
                              # only sane default — false would mean always overwriting to
                              # an exact version, knope-style, which this design rejects)

[validation]
allow-empty-changesets = false   # §6.3 — misfiled under [cascade] in earlier drafts; this is
                                  # a validation setting, not a cascade one

[[package-set]]
match = "crates/*"
release-trigger = "auto"
publish-to = ["cratesIo"]
pre-major-inference = "conservative"   # §7.1 — opt-in remap of inferred (not explicit)
                                        # severities for Auto-trigger packages at 0.y.z,
                                        # y > 0; omit/false to leave inference unremapped.
                                        # Available on [[package-set]]/[[package]]/
                                        # [[fixed-group]]/[[linked-group]] blocks alike —
                                        # shown here since Rust/0.x crates are the case P1's
                                        # §4 scope note and this key exist for

[[package-set]]
match = "packages/*"
release-trigger = "changeset"
publish-to = ["npm"]

[[package]]
match = "packages/special-case"
release-trigger = "changeset"   # explicit [[package]] entries always win over set matches
publish-to = ["npm"]

[[fixed-group]]
name = "napi-foo"
members = ["foo", "foo-cli"]   # or auto-derived, see §5.3

[[linked-group]]
name = "foo-and-its-cli-docs"
members = ["foo", "foo-docs"]   # share a version only when jointly touched (§7.5) —
                                 # previously defined in §7.5 as a first-class peer of
                                 # fixed-group but never given a config block

[registries.cratesIo]
type = "cargo"
[registries.npm]
type = "npm"
```

```yaml
# packages/foo/moon.yml
extensions:
  callisto:
    package-name: foo
    release-trigger: changeset
    publish-to: [cratesIo, npm]
    tag-template: "foo@{version}"
```

Discovery walks the workspace (moon's project graph when available for authoritative
project roots, `ignore`-crate walk otherwise — `ProjectLocator`, §0.1/§15); `package-set`
matches expand to concrete packages; explicit `[[package]]` entries always win over set
matches. A set matching nothing, or two sets claiming the same package, is a hard config
error. **Group membership validation, at parse time (P5), not at mutation time**: a package
must belong to at most one `[[fixed-group]]` and at most one `[[linked-group]]`, and the
fixed and linked member sets must be disjoint — reject with a clear error listing the
conflicting groups, rather than letting an ambiguous membership surface as a confusing
mutation-time alignment failure.

---

## 15. Architecture

**Resolved (§0.1): Option C, narrowly defined.** Crate layout:

```
callisto/
├── crates/
│   ├── callisto-format/       # MIT/Apache-2.0 — changeset + pre.json parser, zero deps
│   │                            on workspace/moon concepts. The primitive worth spreading.
│   ├── callisto-model/        # MIT/Apache-2.0 — Package, ManifestDecl, DepSpec, Ecosystem,
│   │                            Severity, DepEdge, DepKind, DependencyEntry, WorkspaceKind,
│   │                            DeclaredEdge/DeclaredEdgeKind (moon cross-check, below),
│   │                            and the plan/report value types (§9.2) — moved here from an
│   │                            earlier draft that left them in AGPL-tier callisto-graph;
│   │                            they're the public JSON contract per P7, so MIT/Apache tier.
│   ├── callisto-graph/        # AGPL-3.0 — dependency graph, cascade, groups, ProjectLocator
│   │                            Two traits, not one: `DependencyResolver` (edges + specs —
│   │                            ManifestWalkResolver is the sole real-world impl; a second,
│   │                            in-memory impl lives in the dev-only `callisto-fixtures`
│   │                            crate below, which is why the trait exists at all rather
│   │                            than being a concrete struct) and `ProjectLocator`
│   │                            (project discovery — IgnoreWalkLocator here,
│   │                            MoonProjectLocator in callisto-moon). No
│   │                            `MoonProjectGraphResolver`: moon's project-graph edges
│   │                            carry no version-requirement string (verified against
│   │                            moon's actual `ProjectDependencyConfig`), so they cannot
│   │                            supply what §7.4's cascade needs — moon's edges are only
│   │                            ever a cross-check (`declared_edges()`, below), never a
│   │                            resolver.
│   ├── callisto-manifests/    # AGPL-3.0 — per-ecosystem read/write, feature-flagged
│   ├── callisto-conventional/ # AGPL-3.0 — commit parsing for auto-mode (v0.2, §17)
│   ├── callisto-changelog/    # AGPL-3.0 — changelog generation (v0.1, §17 — previously
│   │                            unscheduled despite v0.1's `version` command needing it)
│   ├── callisto-cli/          # AGPL-3.0 — standalone binary; argv parsing, rendering,
│   │                            process I/O only. CI-enforced: no graph construction or
│   │                            cascade code in this crate (§13 invariant 27).
│   ├── callisto-moon/         # AGPL-3.0 — WASM extension; MoonProjectLocator lives here,
│   │                            pinned to a specific moon compatibility range, since this
│   │                            trait boundary is expected to break pre-1.0 the same way
│   │                            Nx's `VersionActions` did inside one minor version
│   └── callisto-fixtures/     # dev-only, unpublished — byte-compat corpus, plan-schema
│                                golden files, and the in-memory DependencyResolver impl
│                                shared by callisto-cli's and callisto-moon's test suites
```

The load-bearing design commitment: **`callisto-graph` has zero moon dependency**, CI
(§13 invariant 26) — enforced by the crate boundary, not by convention. moon integration is
the one blessed, first-shipped integration (not a symmetric second API surface); the
boundary being clean is what makes that cheap to be wrong about later, not a bet that a
second integration is coming.

Two supporting types referenced above but not yet declared anywhere: `ProjectRoot { id:
PackageId, path: PathBuf, ecosystem: Ecosystem }` (one located project) and `DeclaredEdge {
from: PackageId, to: PackageId, kind: DeclaredEdgeKind, via: Option<String> }` (moon's `via`
provenance field, carried through for diagnostics) with `enum DeclaredEdgeKind { Build,
Development, Peer, Production, Root }` — deliberately named and shaped after moon's own
`DependencyScope`, not callisto's `DepKind` (§5.1), because the mapping between them is
lossy in both directions (moon has `Root`, no `Optional`; `Production` doesn't cleanly split
into callisto's `Runtime`+`Optional`) and collapsing that distinction by reusing `DepKind`
directly would silently launder the lossiness the decision doc explicitly flagged. `LocateError`
and `GraphError` are ordinary per-crate error enums (`callisto-graph`), not given full
signatures here. (The decision doc's `DeclaredEdge` sketch uses `ProjectId`; that was this
crate's working name during the research phase — `PackageId` throughout, per §5.1, no
`ProjectId` type exists.)

Key traits (`Manifest`, `ProjectLocator`, `DependencyResolver`, `CommandRunner`; the
path-resolution seam is `from_virtual_path`/`to_virtual_path` (§10, moon's own extism host
functions — no separate callisto trait needed, since it's moon-side, not core-side).
`PublishBackend` is gone entirely per §9's scope cut — no `is_published` queries, no publish
calls, since callisto doesn't execute publishes, only plans them):

```rust
// The exec seam (§0.1 rule 2, §11): last_tag_for and Auto-trigger commit inference both
// need `git`. CLI: a real subprocess. callisto-moon: routed through moon's `exec_command`
// host function. No third impl is anticipated; this exists as a trait rather than a direct
// `std::process::Command` call specifically so the core crate compiles for wasm32-wasip1
// (§0.1 rule 2) without a `#[cfg]`-gated call site.
pub trait CommandRunner: Send + Sync {
    fn run(&self, program: &str, args: &[&str], cwd: &Path) -> Result<CommandOutput, CommandError>;
}

pub trait Manifest: Send + Sync {
    fn path(&self) -> &Path;
    fn ecosystem(&self) -> Ecosystem;
    fn role(&self) -> ManifestRole;   // drives §7.6 steps 3/4/5's dispatch (canonical write
                                       // vs. platform inherit vs. optionalDependencies
                                       // update) — the axis §5.2 calls callisto's key
                                       // differentiator; omitted from an earlier draft
    fn package_name(&self) -> Result<String, ManifestError>;
    fn current_version(&self) -> Result<Version, ManifestError>;
    fn write_version(&mut self, v: &Version) -> Result<(), ManifestError>;
    fn iter_dependencies(&self) -> Box<dyn Iterator<Item = DependencyEntry> + '_>;
    fn update_dependency_spec(&mut self, name: &str, kind: DepKind, new: DepSpec)
        -> Result<(), ManifestError>;
    fn update_optional_dependencies(&mut self, updates: &[(String, Version)])
        -> Result<(), ManifestError>;
}

// Discovery: moon is authoritative when present (supersedes the `ignore` walk outright);
// this resolves §18's former Q1a.
pub trait ProjectLocator: Send + Sync {
    fn projects(&self) -> Result<Vec<ProjectRoot>, LocateError>;
    // Non-authoritative cross-check only (§18's former Q1b) — moon's DependencyScope is
    // deliberately NOT reused here (it would leak a moon type into this moon-agnostic
    // crate); DeclaredEdgeKind is callisto-owned with an explicit, documented mapping from
    // moon's Build|Development|Peer|Production|Root, applied by MoonProjectLocator.
    fn declared_edges(&self) -> Option<Vec<DeclaredEdge>> { None }
}

pub trait DependencyResolver: Send + Sync {
    fn packages(&self) -> impl Iterator<Item = &Package>;
    fn dependencies_of(&self, id: &PackageId) -> impl Iterator<Item = &DepEdge>;
    fn dependents_of(&self, id: &PackageId) -> impl Iterator<Item = &DepEdge>;
    fn toposort(&self, subset: &HashSet<PackageId>) -> Result<Vec<PackageId>, GraphError>;
}
```

`callisto-graph`'s Rust API (both traits above) is explicitly unstable pre-1.0, documented
as such in its README — the supported, versioned, fixtured contract is `--format json` on
stdout (§12.5), not this API surface.

---

## 16. License posture

- **`callisto-format`, `callisto-model` — MIT OR Apache-2.0.** Primitive layer, worth
  spreading; being the canonical Rust implementation of the changeset format is more
  valuable than the license lever on a small parser + data model. `callisto-model` includes
  the §9.2 plan/report value types (§15) precisely because they're the public JSON contract
  per P7 — permissively licensed so the Action, a future non-Rust consumer, or anyone
  writing an independent integration can depend on the schema types without touching AGPL
  code.
- **Everything else — AGPL-3.0**, with contributor CLA/copyright assignment to preserve
  dual-licensing optionality. Creates friction against unattributed commercial repackaging
  of the actual coordination logic. This is a supporting consideration for §0.1's Option C,
  not a load-bearing one — AGPL lowers the expected number of *third-party* consumers of
  `callisto-graph`/`callisto-manifests`, which lowers the value of paying for a stable
  public Rust API there, but it says nothing about where the seams themselves belong; that
  argument rests entirely on the Nx/knope/semantic-release evidence in
  `docs/02-library-vs-moon-decision.md`, not on licensing.

---

## 17. Milestones

**Only v0.1–v0.4 are committed.** Everything past that is demand-gated per §2.2 — the
scope discipline this revision imposes.

- **v0.1 — Rust + npm core.** `callisto-format` (byte-compat fixtures), `callisto-model` +
  `callisto-graph` (§7's full dependency graph and cascade table, §7.4 — all six
  edge-kind/coverage rows, including peer-escalation and dev-none, ship here since neither
  is napi-specific; a prior draft deferred them to v0.3 alongside groups, which was wrong —
  groups are napi-specific, the cascade table isn't), `callisto-manifests` (Cargo.toml +
  package.json, range preservation, `WorkspaceCargoResolver` for `[workspace.dependencies]`
  inheritance — committed here, not merely "must be handled from v0.1" in an open-questions
  aside), `callisto-changelog` (changelog generation — §7.6 step 7 is a mutation step of
  `version`, which ships here; previously unscheduled in any milestone despite this
  dependency), `callisto-cli` with `add`/`status`/`version`/`init`. Stateless `last_tag_for`
  primitive (§9.1's glob-and-extract resolution) built and used by `status` even before
  `plan-publish` ships. `init` doubles as the reconcile flow on an already-initialized
  workspace (§18 Q5.4) — not deferred, since the napi group-promotion path in v0.3 depends
  on this diff-reviewed mechanism already existing. napi *detection* (reading
  `napi.targets`, refusing with an explicit "platform coordination ships in v0.3" message)
  ships here too, ahead of napi *coordination* itself (§18 Q5.6) — without it, a v0.1 user
  with a napi workspace would get silently wrong platform versions.
- **v0.2 — Publish planning + CI outputs.** `plan-publish` (§9), `compose-pr-body`,
  `callisto tag` (§9.1 — the sole sanctioned tag-creation path once tag ownership was
  resolved; ships alongside `plan-publish` since neither is useful without the other in a
  real publish workflow, §9.3), `--format json` with schemaVersion, empty-changeset
  validation (§6.3), `validate` subcommand (§18's former Q3 — closed: dedicated subcommand,
  `--staged`/`--since <ref>`, not a flag on other commands, since §11/§12.2 already assumed
  this shape), `snapshot` subcommand (§8), `callisto-conventional` (conventional-commit
  parsing for `ReleaseTrigger::Auto`, §5.1/§7.1 — previously undeclared in any milestone
  despite the `Auto` variant existing in `callisto-model` since v0.1; without this crate
  `Auto`-trigger packages have a type but no working inference until v0.2).
- **v0.3 — napi.** `Platform` manifest role + auto-derivation, fixed/linked groups with
  alignment checks (§7.5, including the fixed-∩-linked parse-time validation and the
  `napi.targets` drift cross-check), pre-mode (§8, including its fixed-group and
  `Auto`-trigger interactions).
- **v0.4 — moon integration + the Action.** `callisto-moon` WASM extension,
  `orin-dx/callisto-action` (single composite, modes per §12.2), `setup-moon-callisto`,
  migration guide from `@changesets/cli` (including `callisto init`'s
  `.changeset/config.json` translation, §18 Q4).
- **Beyond v0.4 — demand-gated only.** Python, Go, JVM, NuGet, Deno/JSR, Ruby, a proto
  plugin, an npm installer wrapper, a GitHub App alternative to the Action. None of these
  are scheduled; each ships if and when a real user asks, using the trait boundaries in §7
  and §15 that were designed to make each one bounded work.

---

## 18. Open questions

Ranked by how much they block forward progress. **Q0–Q3 are resolved/committed** (kept here,
struck to "resolved," rather than deleted, so the resolution's rationale stays discoverable
from the same place the question was originally raised):

**Q0 (resolved) — Library-first, moon-first, or hybrid?** Option C, narrowly defined. See
§0.1 and `docs/02-library-vs-moon-decision.md` for the full argument.

**Q1 (resolved) — Does `callisto-graph` treat moon's project graph as authoritative or as a
cross-check against its own manifest walk?** Split in two, per §0.1/§15: **Q1a** (project
discovery) — moon is authoritative when present, via `ProjectLocator`. **Q1b** (dependency
edges) — cross-check only, via `declared_edges()`, because moon's edges carry no
version-requirement string and can never be a resolver, only a diagnostic.

**Q2 (resolved, committed to v0.1) — `Cargo [workspace.dependencies]` inheritance.** Modern
Cargo workspaces define versions once at the root; members inherit via `foo.workspace =
true`. Bumping a member's dep means editing the root, not the member. `WorkspaceCargoResolver`
in `callisto-manifests`, §17 v0.1 — this used to say "must be handled correctly from v0.1"
while sitting in this open-questions list and absent from the actual v0.1 milestone bullet;
it's committed there now, not just asserted here.

**Q3 (resolved) — `validate` as a subcommand or a flag on other commands?** Dedicated
subcommand (`callisto validate --staged`, `--since <ref>`) — §11 and §12.2 already assumed
this shape before the question was formally closed; §17 v0.2 now ships it explicitly.

**Q4 (resolved) — Migration from `.changeset/config.json`**

**Shape: `callisto init` detecting an existing `.changeset` directory, not a standalone
`callisto migrate` subcommand.** A dedicated subcommand would have exactly one legitimate
invocation per repo, ever — once at adoption, then dead weight in the CLI's surface for the
rest of the project's life. `callisto init` already owns this class of decision: §5.3
specifies the identical UX pattern for napi group derivation ("`callisto init` offers to
write the derived group into `callisto.toml` as explicit config on first run"), and §10 gives
`init` a host-side prompt surface via `initialize_extension` that a new subcommand would have
to duplicate or route around. Folding migration into `init` also keeps it on the WASM surface
for free (§11: `init` is already WASM-compatible because `initialize_extension` supplies the
prompt seam) rather than forcing a fresh CLI-vs-WASM classification for a new command. Against
§4's principles: P5 (structural fixes, not a README step a user has to remember to run) favors
detection over a command a migrating user has to know exists and invoke correctly; P1 (byte-
compat as the adoption gate) is served either way, since neither shape touches the files P1
guarantees compatibility for — but folding into `init` means the byte-compat promise and the
config-translation offer are presented to the user in the same breath, at the one moment
they're evaluating whether to adopt at all. Concretely: `callisto init`, on finding
`.changeset/config.json`, reads it, produces the translated `callisto.toml` block described
below as a diff for review (never a silent write — same reviewability contract as §5.3's
group-derivation offer and §13 invariant 21's "membership changes are only ever written via
an explicit, reviewable flow"), and proceeds with normal scaffolding if the user declines.

**Confirmed: migration touches `config.json` only, never `.changeset/*.md` or `pre.json`.**
This is the reason the surface is small at all. Per P1, the changeset Markdown files and
`pre.json` are already byte-compatible with what callisto reads and writes (§6, §6.4) — there
is nothing to translate in either, and `callisto init`'s migration step does not open, parse,
or rewrite a single one of them. The only artifact requiring translation is `config.json`,
because it's the one part of `@changesets/cli`'s on-disk footprint that encodes *settings*
(changesets' own tool configuration) rather than *data* (the changeset records themselves),
and settings are exactly what §14's `callisto.toml` schema exists to hold under a different
shape. `.changeset/config.json` itself is left in place, untouched, after migration — deleting
it is not this step's job and would gain nothing, since an unrecognized file alongside
`callisto.toml` is inert.

**Field-by-field mapping**, against `@changesets/cli`'s real `config.json` schema (the
`@changesets/config` package's schema, as of the version this design was researched against)
and §14's `callisto.toml` keys:

| `config.json` field | Shape | `callisto.toml` mapping | Notes |
|---|---|---|---|
| `changelog` | `string \| [string, options] \| false` | **No equivalent — dropped, with a warning naming the value being dropped.** | `callisto-changelog` (§17 v0.1) is a single built-in generator; §14 has no changelog-selection key because there is no plugin architecture behind it (unlike changesets' `getReleaseLine`/`getDependencyReleaseLine` package resolution). A value like `["@changesets/changelog-github", {"repo": "org/repo"}]` means PR/commit-linking behavior will not carry over — the warning says so explicitly, not just "dropped." |
| `commit` | `boolean \| [string, options]` | **No equivalent — dropped, with a warning, unconditionally on any truthy value.** | Directly contradicts §7.6 step 11 ("Stage modified files to git. Never commit.") and P2's git-write discipline. Even a faithful translation would be a config key callisto would refuse to honor, so there is no version of this mapping worth writing — warn and move on. |
| `fixed` | `string[][]` | One `[[fixed-group]]` block per inner array | `members` is the resolved package-name list; `name` is synthesized (`fixed-1`, `fixed-2`, ... or derived from the first member) since `config.json`'s arrays carry no name for `init` to reuse. |
| `linked` | `string[][]` | One `[[linked-group]]` block per inner array | Same translation shape as `fixed`; §14's parse-time disjointness check (a package in at most one fixed group and at most one linked group, the two sets disjoint) runs on the translated output exactly as it would on hand-written config, so a `config.json` that (invalidly, but changesets doesn't reject it) put the same package in both `fixed` and `linked` surfaces as a hard `init`-time error, not a silent pick-one. |
| `access` | `"public" \| "restricted"` | **No equivalent — dropped, with a warning pointing at the publish step.** | §9's versioning-coordinator posture means callisto never runs `npm publish`; access level is an argument to whatever `npm publish`/`pnpm publish` invocation the calling workflow already owns (§9.3's worked example), not a value `callisto.toml` has a slot for. |
| `baseBranch` | `string` | **No equivalent — dropped, with a warning.** | callisto's analogous inputs — `validate --since <ref>` (§17 v0.2, §18 Q3) and inference's commits-since-tag walk (§7.1) — are explicit per-invocation arguments or tag-derived, never a persisted default, consistent with P2 (statelessness for correctness). The warning suggests passing `--since <baseBranch>`'s former value explicitly in whatever CI step used to rely on `config.json`'s default. |
| `updateInternalDependencies` | `"patch" \| "minor"` | `[cascade].bump-severity` | Direct, lossless match — same semantic axis (§7.4, §14): how hard a dependent bumps once cascade fires, distinct from whether it cascades at all (`[cascade].mode`, which `config.json` has no equivalent knob for and `init` leaves at its default, `"out-of-range"`). |
| `ignore` | `string[]` | **Translated by omission, not a key.** | Named packages get no `[[package]]`/`[[package-set]]` entry in the generated `callisto.toml` — §14's model treats "not matched by any set or explicit entry" as "not part of the release process," which is the same effect `ignore` has in changesets. Because omission is invisible in a config-file diff, `init` lists the omitted package names in its migration summary output so the user can confirm the exclusion was intentional rather than discovering it later as a package that silently never gets a changeset prompt. |
| `privatePackages.version` | `boolean` | Package included as a normal `[[package]]`/matched `[[package-set]]` entry, with no `publish-to` targets (`publish-to = []`, or the key omitted) | `version: true` means "still bump this package's version, it's just never published" — exactly `PublishTarget::None`'s meaning (§5.1: "internal-only, never published"). `version: false` maps the same way `ignore` does — omitted entirely, with the same summary-line disclosure. |
| `privatePackages.tag` | `boolean` | **No equivalent — dropped, with a warning.** | Tag-bearing status in callisto is not a per-package boolean; it's structural — whether a package appears in `plan-publish`'s `releases[]` (§9.2), which is a consequence of being a real release point (a napi main package, or a crate/npm package separately published), not a flag layered on top. There is no `callisto.toml` key this could set even in principle, so the translation is a pure drop. |
| `___experimentalUnsafeOptions_WILL_CHANGE_IN_PATCH` (and any other `changesets`-internal experimental key) | any | **Dropped silently — no warning.** | Unlike the fields above, this is changesets' own explicitly-unstable internal surface, not a stable part of the schema being migrated away from; warning about it would just be noise. |

**Milestone: v0.4, not v0.2 — the stub's suggestion was wrong, moved here.** §17 v0.2
("Publish planning + CI outputs") is thematically about `plan-publish`, `compose-pr-body`,
`callisto tag`, and the `--format json` contract — none of it is about first-run adoption
tooling, and `callisto init` itself ships in v0.1 with no dependency on anything v0.2 adds.
§17 v0.4 already commits to "a migration guide from `@changesets/cli`" as part of shipping
`callisto-moon`/`orin-dx/callisto-action`/`setup-moon-callisto` — the milestone where a team
actually completes a switch-over, since that's when the Action and moon integration exist to
switch *to*. Shipping automated `config.json` translation two milestones before the
human-readable migration guide it's meant to accompany would split one adoption story across
two releases for no benefit — a team migrating in v0.2 would have `callisto init`'s config
translation but no Action, no `callisto tag`, and no guide explaining the rest of the move.
Landing both in v0.4 means the guide can reference the automated step (and vice versa) as one
coherent piece of adoption collateral. This resolves Q4 as: **fold into `callisto init`,
config.json-only, ships in v0.4 alongside the migration guide** — §17's v0.4 bullet should
read "...`setup-moon-callisto`, migration guide from `@changesets/cli` (including `callisto
init`'s `.changeset/config.json` translation, §18 Q4)."

## Q5 (resolved) — Progressive complexity

**Verdict: nothing in §17's committed scope moves in substance, and no config key is added
or removed beyond one new attribution field. What Q5 actually needed was a specification of
`callisto init`'s detection and write rules, plus one structural rule about diagnostic
output (§13 invariant 28) and one small v0.1 addition (napi *detection* without napi
*coordination*). The premise that callisto's surface is much larger than
`@changesets/cli`'s is also partly false, and worth deflating before designing around it —
see the concept-inventory below.**

### Q5.1 The premise, audited

`@changesets/cli`'s own `init` writes a nine-key `.changeset/config.json` (`$schema`,
`changelog`, `commit`, `fixed`, `linked`, `access`, `baseBranch`,
`updateInternalDependencies`, `ignore`). Fixed and linked groups are *changesets' own
concepts*, not callisto additions — §7.5 names them the way it does precisely because P1
makes them the same feature. Counting only genuinely new concepts:

| callisto surface | `@changesets/cli` equivalent | New concept? |
|---|---|---|
| `[changesets].dir` | the `.changeset/` directory itself | No |
| `[cascade].bump-severity` | `updateInternalDependencies` (`patch`\|`minor`) | No — renamed, same axis, same values |
| `[cascade].mode` | `___experimentalUnsafeOptions_WILL_CHANGE_IN_PATCH.updateInternalDependents` (`always`\|`out-of-range`) | No — §7.4's value names are that option's, promoted out of experimental |
| `[[fixed-group]]` / `[[linked-group]]` | `fixed` / `linked` | No |
| `[[package-set]]` / `[[package]]` | `ignore`, plus per-package `access` | Partly — glob-set matching is new, the intent isn't |
| `[cascade].peer-escalation` | none (behavior is hardcoded) | Yes |
| `[cascade].preserve-npm-ranges` | none (always on, not configurable) | Yes, as an off-switch §14 already declines to recommend |
| `[validation].allow-empty-changesets` | none (§6.3's check doesn't exist there) | Yes |
| `release-trigger` (`Auto`) | none | Yes — v0.2 |
| `pre-major-inference` | none (release-please's `bumpMinorPreMajor`) | Yes — v0.2 |
| `tag-template` | none (tag shape is fixed) | Yes — but §9.1's default *is* changesets' shape |
| `publish-to` / `[registries.*]` | `access` + `.npmrc` | Roughly |
| `ManifestRole::Platform` / napi coordination | none | **Yes — this is the wedge (§2.1 cases E/F)** |

Every "yes" row above is either off by default, inert until a later milestone, or
napi-specific. The net new concepts a Cargo+npm user must hold in their head on day one is
**zero**. Q5's real risk is therefore not surface size; it is that `init` might *write* that
surface into `callisto.toml` and thereby make it look mandatory. The levels below exist to
constrain what `init` writes, not to constrain what callisto does.

### Q5.2 The governing rule

> **`callisto init` writes a key only when the workspace's answer differs from callisto's
> built-in default, or when the answer is not derivable from the manifests on disk.
> Everything else is a comment or is absent.**

Derivation rules `init` relies on, so that they never need to be written:

- `publish-to`: `Cargo.toml` → `["cratesIo"]`, unless `publish = false` (→
  `PublishTarget::None`) or `publish = ["<alt>"]` (→ that registry). `package.json` →
  `["npm"]`, unless `"private": true` (→ `None`) or `publishConfig.registry` is set (→
  `Npm { registry }`).
- `release-trigger`: `changeset` — the byte-compat-shaped default (§5.1), and the only one
  implemented before v0.2.
- Package identity: bare (§5.4), promoting to `ecosystem/name` only for names that actually
  collide across ecosystems; `callisto add` emits the shortest unambiguous form, so a user
  meets prefixed identity by seeing it written for them, not by reading about it.
- Case D collapse: co-located `Cargo.toml` + `package.json` at one project root are one
  `Package` with two `Canonical` manifests (§5.2), by structural default. The *divergent*
  case — two independently-versioned sides — is the one that costs explicit config (§5.4's
  "model them as two separate `Package`s"). Cheap default, expensive exception, in that
  order.
- `[registries.*]`: `cratesIo`/`npm` are implicit; a block is written only for a third
  registry.

### Q5.3 The levels

| Level | §2.1 case | Ships | Keys the user must type | What they must hold in their head |
|---|---|---|---|---|
| **L0** | A or B | v0.1 | 0–1 | Exactly `@changesets/cli`'s model: write a changeset, run `version` |
| **L1** | C, D | v0.1 | 0 | The above, plus "one changeset stream covers both ecosystems" |
| **L2** | any | v0.1 (`[cascade]`, `[validation]`), v0.2 (`release-trigger = "auto"`) | 1–4 | Cascade is a policy with two independent axes |
| **L3** | E, F | v0.3 | 2–3 per group | Groups; napi main+platform lockstep |
| **L4** | any | v0.2 (`snapshot`, `pre-major-inference`), v0.3 (pre-mode) | 0–2 per package | Version-line policy: pre-release cycles, tag shape, 0.x inference |

**L0 — single ecosystem, zero policy config.** A pure Cargo or pure npm workspace.
Exercised feature surface: §6's format, §7.1 aggregation (`Changeset` trigger only), §7.2's
manifest-walk graph, §7.4's cascade table at its defaults, §7.6's mutation ordering. The
user never types the words "cascade," "group," or "trigger." `callisto.toml` contains
`[changesets] dir` and a comment header. For a team migrating from `@changesets/cli`, Q4's
migration path (v0.4) is the second on-ramp to this same level.

**L1 — Cargo + npm, still zero policy config.** Case C is two disjoint graphs sharing one
changeset stream and one `version` command — there are no cross-ecosystem edges, by
definition of "disjoint," and none need configuring. Case D introduces the first genuine
cross-registry edge and the first prefixed identity, both handled by the derivation rules
above. **`init` writes no `[[package-set]]` blocks at this level**, because §14's example
blocks only restate what the manifests already say; writing them would teach a user that
per-ecosystem config is mandatory when it is not.

**L2 — cascade and trigger policy.** The first level where config changes *behavior* rather
than describing *structure*: `[cascade].mode = "always"` for teams that want forced
downstream republishes, `[cascade].bump-severity = "minor"`, `[cascade].peer-escalation =
false`, `[validation].allow-empty-changesets`, and (v0.2) `release-trigger = "auto"` with
`[[package-set]]` globs. `init` never scaffolds this level — it has no signal that would
justify a guess — but the *diagnostics* route users here (Q5.4, mechanism 2).

**L3 — groups and napi.** `[[fixed-group]]` for napi main+platform lockstep (§5.3, §7.5),
`[[linked-group]]` for usually-but-not-always-together version lines. This is the only
level `init` scaffolds beyond L1, and only for fixed groups, and only from the
`napi.targets` signal. Membership stays config-authoritative forever after (§13 invariant
21).

**L4 — version-line policy.** `callisto pre enter/exit` and `callisto snapshot` are
**zero-config** — pre-mode's entire state lives in `pre.json`, a file created by a command,
byte-compatible with changesets (§6.4, §8). The config-bearing members of this level are
`tag-template` (per-package, `moon.yml`), `pre-major-inference` (§7.1), and non-default
`[registries.*]`.

**On "per-ecosystem grammar overrides."** Q5's original phrasing overstates this by a full
level: for the whole committed v0.1–v0.4 scope, both ecosystems are SemVer, so §7.7's
`Versioning` trait is selected by `Ecosystem` and **has no user-facing override key at
all**. The three settings that *feel* grammar-shaped — `tag-template`,
`pre-major-inference`, `preserve-npm-ranges` — are all L4, all default-inert, and none of
them changes a version grammar. There is nothing here for `init` to scaffold, now or at
v0.4.

### Q5.4 Graduation — three mechanisms, not one

The question "is it automatic or does the user have to know to ask?" has three different
answers depending on whether the feature has an on-disk signal, a behavioral tell, or
neither.

**Mechanism 1 — detected and offered (structural signal exists).** Re-running `callisto
init` is the reconcile flow; it re-detects, prints a diff, and applies only with
confirmation (`--yes` for CI). Signals: a `package.json` appearing in a Cargo-only
workspace (L0→L1), a co-located manifest pair appearing (Case D), a new cross-ecosystem name
collision, `napi.targets` appearing or changing (L1→L3). Between `init` runs, `status`/
`version` surface the same drift as warn-by-default cross-checks, `--strict`-escalatable,
and **never** auto-mutate `callisto.toml` (§5.3, §7.5, §13 invariant 21). This resolves
§5.3's "a future `--sync` variant" as *not a separate variant*: `init` on an
already-initialized workspace **is** the sync flow, and it is idempotent per P3.

**Mechanism 2 — attributed at the point of surprise (behavioral tell, no structural
signal).** This is how a user discovers L2. Every decision whose default could defensibly
have gone the other way names its own governing config key inline when it fires:

```
  @myorg/cli   1.4.2 → 1.4.3   cascade: dep @myorg/sdk 1.2.0 → 2.0.0, out of range "^1.2"
                               governed by [cascade].bump-severity = "patch" (default)
  @myorg/host  3.1.0 → 4.0.0   cascade: peer dep @myorg/sdk bumped minor-or-worse
                               governed by [cascade].peer-escalation = true (default, §13 inv. 9)
  crates/tooling                spec rewrite only, no bump (dev-dep, §7.4)
```

The user learns `[cascade].bump-severity` exists at exactly the moment they want it to be
different — which is the only moment the knowledge is worth anything. This is P5 applied to
documentation: a config key discoverable only by reading the docs is a design defect, not a
docs gap. It becomes **§13 invariant 28**.

**Mechanism 3 — the user has to ask, and that is correct.** Linked groups, `release-trigger
= "auto"`, `pre-major-inference`, `tag-template`, pre-mode, snapshot. These encode *release
intent*, which by construction leaves no trace on disk — there is nothing to detect, and a
tool that guessed would guess wrong. The structural mitigation is not discoverability but
inertness: **every feature above the level a user is on is inert by default, so the cost of
never learning it exists is zero.** That claim is what makes the tiering sound rather than
merely tidy, so it gets audited rather than asserted:

- `[cascade].mode = "out-of-range"` — the minimal-cascade choice. Inert.
- `[cascade].bump-severity = "patch"` — the minimal bump. Inert.
- `release-trigger = "changeset"` — no inference runs. Inert.
- `pre-major-inference` off, `snapshot`/pre-mode not entered, no groups declared. Inert.
- §6.3's empty-changeset check and §7.5's `napi.targets` drift check warn by default;
  neither changes an outcome. Inert for results.
- **`[cascade].peer-escalation = true` — not inert, and deliberately so.** An L0 user *can*
  be surprised by a major bump they never configured. This is the one sanctioned exception,
  because the alternative default is silent under-bumping, which is a correctness bug
  rather than a surprise (§13 invariant 9's "opt-out not opt-in"). Mechanism 2 is what makes
  the exception survivable: the escalation line names the key that caused it.

One exception, named and justified. No others.

### Q5.5 First run, concretely

Workspace:

```
myrepo/
├── Cargo.toml                    # [workspace] members = ["crates/*"]
├── crates/engine/Cargo.toml
├── crates/engine-macros/Cargo.toml
├── package.json                  # "workspaces": ["packages/*"]
├── pnpm-lock.yaml
├── packages/sdk/package.json     # @myorg/sdk
└── packages/cli/package.json     # @myorg/cli, deps: { "@myorg/sdk": "^1.2.0" }
```

```
$ callisto init
callisto 0.1.0 — initializing /Users/me/myrepo

Discovery
  project locator      ignore-walk (moon not detected)
  cargo workspace      Cargo.toml [workspace] → 2 members
  npm workspace        package.json "workspaces" → 2 members (pnpm-lock.yaml)

Packages (4)
  engine               crates/engine/Cargo.toml           → cratesIo
  engine-macros        crates/engine-macros/Cargo.toml    → cratesIo
  @myorg/sdk           packages/sdk/package.json          → npm
  @myorg/cli           packages/cli/package.json          → npm

Structure
  dual-published       none — no co-located Cargo.toml + package.json
  name collisions      none — bare names are unambiguous workspace-wide
  napi packages        none — no "napi" key in any package.json
  internal edges       2 runtime (engine→engine-macros, @myorg/cli→@myorg/sdk), 0 cross-ecosystem

Wrote
  callisto.toml        1 setting, 0 policy overrides — defaults cover this workspace
  .changeset/          created, with .changeset/README.md
  .gitignore           + ".callisto/"

Not written, deliberately
  [cascade]            defaults apply — mode=out-of-range, bump-severity=patch,
                       peer-escalation=on, preserve-npm-ranges=on
  [[package-set]]      publish targets derive from each manifest; nothing to override
  [[fixed-group]]      no napi platform packages detected
  [[linked-group]]     release intent — callisto cannot detect this; declare one if two
                       packages should share a version only when jointly released (§7.5)

Next
  callisto add         record a change
  callisto status      see what a release would do
```

`init` does **not** write `.changeset/config.json`; that file is `@changesets/cli`-owned,
and if it already exists `init` leaves it byte-untouched and points at the migration path
(Q4, v0.4). `init` supports `--format json` (its output is the moon-side payload for
`initialize_extension`, §10/§11) and `--yes` for non-interactive use.

**`callisto.toml` after `init`, L1 (this workspace):**

```toml
# callisto.toml — generated by `callisto init` 0.1.0 on 2026-07-24.
#
# Everything not listed here uses callisto's defaults. That is the intended
# steady state for a workspace this shape: 2 Cargo crates, 2 npm packages,
# no dual-published packages, no napi platform packages.
#
# Re-run `callisto init` after adding a package, adding an ecosystem, or
# changing napi.targets: it re-detects, shows you a diff, and never rewrites
# a key you have edited.

[changesets]
dir = ".changeset"
```

**`callisto.toml` at L3**, the same workspace after a napi package lands in
`packages/native/` (crate + main JS package co-located, three platform targets), the team
has moved Rust crates to conventional-commit inference, and has decided every dependent
should republish on every release:

```toml
[changesets]
dir = ".changeset"

# L2 — added by hand after `callisto version` attributed a surprising bump to
# [cascade].bump-severity (§13 inv. 28).
[cascade]
mode = "always"

[[package-set]]
match = "crates/*"
release-trigger = "auto"

# L3 — derived by `callisto init` 0.1.0 from packages/native/package.json's
# "napi.targets" and accepted as a reviewed diff (§5.3). From here on this list
# is authoritative: callisto cross-checks it against napi.targets every run and
# warns on drift (--strict to hard-fail), but never rewrites it (§13 inv. 21).
[[fixed-group]]
name = "napi-native"
members = [
  "@myorg/native",
  "@myorg/native-darwin-arm64",
  "@myorg/native-linux-x64-gnu",
  "@myorg/native-win32-x64-msvc",
]
```

Three blocks, each of which the user either asked for or accepted as a diff. Nothing about
groups, cascade modes, or triggers was required to get from `git clone` to a first shipped
release.

### Q5.6 Effect on committed scope (§17)

Purely `init`-behavioral, with three small, named exceptions — no feature moves between
milestones, no §7 semantics change, and no config key is added beyond one attribution
field:

1. **napi *detection* moves to v0.1; napi *coordination* stays v0.3.** `init` and `status`
   must read `napi.targets` and say, explicitly, that platform coordination ships in v0.3
   and that until then only the main package is versioned. Without this, a v0.1 user with a
   napi workspace gets silently wrong platform versions — the exact class of failure P5
   exists to make structural. The cost is reading one JSON key and printing a refusal; it is
   not the `Platform` manifest role, which stays in v0.3 with the rest of §7.5.
2. **§13 invariant 28 (new), v0.1.** *Every default that fires and could defensibly have
   gone the other way names its governing config key in human-readable output; the
   corresponding plan/report value type carries an optional attribution field.* — P5, P7
   (§Q5.4 mechanism 2). This is a real v0.1 addition: the cascade and aggregation steps must
   carry a governing-key attribution alongside each decision (computed in `callisto-graph`,
   rendered by `callisto-cli`, per P6), and `--format json`'s `.bumps[]` gains an
   **optional** `governedBy` field — optional so it length-gates rather than hard-gates in
   §12.5's contract.
3. **`init`'s reconcile mode is v0.1, not deferred.** §5.3's "a future `--sync` variant of
   it" resolves to: re-running `init` *is* the sync flow. At v0.1 there is almost nothing to
   reconcile (ecosystem appearance, package-set drift), which is exactly why it is cheap to
   build then — deferring it to v0.3 would leave the napi group-promotion flow with no
   established, idempotent, diff-reviewed path to hook into, and that path is load-bearing
   for §13 invariant 21.

Everything else Q5 raised is satisfied by features already scheduled where they are: the
cascade table and `[cascade]` keys in v0.1, `Auto`/`pre-major-inference`/`snapshot`/
migration in v0.2/v0.4, groups and pre-mode in v0.3.

---

## Appendix A — Reference links

- `@changesets/cli`: https://github.com/changesets/changesets
- Changesets detailed explanation: https://github.com/changesets/changesets/blob/main/docs/detailed-explanation.md
- Changesets polyglot issue: https://github.com/changesets/changesets/issues/665
- Luke Hsiao, "Using Changesets in a polyglot monorepo" (2026): https://luke.hsiao.dev/blog/changesets-polyglot-monorepo/
- `release-please`: https://github.com/googleapis/release-please
- `release-please` manifest + plugin docs: https://github.com/googleapis/release-please/blob/main/docs/manifest-releaser.md
- `release-please` Rust workspace issue (Node-wrapping-Rust case): https://github.com/googleapis/release-please/issues/2207
- Nx Release: https://nx.dev/docs/guides/nx-release
- Nx Release + Rust crates: https://nx.dev/docs/guides/nx-release/publish-rust-crates
- `knope`: https://github.com/knope-dev/knope
- `cargo-release`: https://github.com/crate-ci/cargo-release
- `cargo-semver-checks`: https://crates.io/crates/cargo-semver-checks
- Moon extensions: https://moonrepo.dev/docs/guides/extensions
- napi-rs release docs: https://napi.rs/docs/deep-dive/release
- Conventional Commits spec: https://www.conventionalcommits.org/en/v1.0.0/
