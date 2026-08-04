# Callisto Specification: Python Engine (`docs/04-python-engine-spec.md`)

---

## 1. Specification Overview & Standard Compliance

The Python Engine in Callisto provides native versioning, dependency tracking, and publication orchestration for Python projects in monorepos.

It adheres to 5 core Python Enhancement Proposals (PEPs):
- **PEP 440**: Version Specifiers (`0.3.2`, `0.3.2a1`, `0.3.2.post1`, `0.3.2.dev1`).
- **PEP 508**: Dependency Requirement Specifiers (`requests[security]>=2.28.0; sys_platform == 'win32'`).
- **PEP 517 / PEP 518**: Build isolation & build-system requirements (`maturin`, `hatchling`, `flit_core`, `setuptools`, `poetry-core`).
- **PEP 621**: Standardized `[project]` metadata table in `pyproject.toml`.
- **PEP 735**: Dependency Groups (`[dependency-groups]`). **STATUS: [PLANNED: SPECIFIED]** — see Section 7. `callisto-manifests::python` does not yet read or write `[dependency-groups]`; treat this PEP as a design target, not a shipped compliance claim.

---

## 2. Manifest Schema & CST Editing Rules

`PyprojectToml` in `callisto-manifests::python` uses `toml_edit::DocumentMut` for 100% comment, whitespace, and key order preservation.

### Supported Manifest Standards:
1. **PEP 621 Standard**: `[project] version = "..."`, `[project] dependencies = [...]`
2. **Poetry Format**: `[tool.poetry] version = "..."`, `[tool.poetry.dependencies]`
3. **Flit Format**: `[tool.flit.metadata] version = "..."`
4. **Hatch Format**: `[project] version = "..."`
5. **Maturin Format**: `[build-system] build-backend = "maturin"`, `[project]`

### Version Bumping Rules:
When updating `version`:
- Decor (comments, inline suffix comments) attached to the `version` field MUST be preserved.
- UTF-8 BOM (`\u{FEFF}`) headers MUST be stripped on parse and handled cleanly.

---

## 3. Dependency Requirement Specifier Partitioning

When parsing requirement strings from PEP 621 arrays or Poetry tables:
1. Environment markers (anything after `;`) MUST be preserved separately during updates.
2. Extras (anything inside `[...]`) MUST be preserved during version updates:
   - Example: `"requests[security]>=2.28.0"` $\rightarrow$ package name: `"requests"`, extras: `"[security]"`, requirement range: `">=2.28.0"`.

---

## 4. Lockfile Auto-Staging

When `callisto version` runs, the engine auto-detects and stages the following lockfiles if present in the workspace:
- `uv.lock` (UV package manager)
- `poetry.lock` (Poetry package manager)
- `pdm.lock` (PDM package manager)
- `Pipfile.lock` (Pipenv)

---

## 5. Python Workspace Discovery — **STATUS: [PLANNED: SPECIFIED]**

Callisto's project discovery pipeline (Stage 1 of the pipeline described in `ARCHITECTURE.md`) already runs `IgnoreWalkLocator`, a generic `.gitignore`-aware directory walk that recognizes `Cargo.toml`, `package.json`, and `pyproject.toml` at any depth under the workspace root and emits a `ProjectRoot` per manifest found. This generic walk is `[LIVE: IMPLEMENTED]` for Python today: any `pyproject.toml` reachable from the workspace root (excluding `target/`, `node_modules/`, `.git/`, `.moon/`, `dist/`) is discovered and its `[project].name` or `[tool.poetry].name` is used as the package identity. What is `[PLANNED: SPECIFIED]` and not yet implemented is workspace-aware discovery that mirrors the precision Cargo and npm already get from `WorkspaceCargoResolver` (`callisto-manifests::cargo`) and `detect_npm_workspace_kind` (`callisto-manifests::npm`) — that is, reading an explicit member/exclude declaration from the workspace root manifest instead of relying solely on an unscoped filesystem walk.

### 5.1. uv-native workspace detection (preferred path)

uv is the only mainstream Python toolchain with a native, Cargo-like workspace concept. When a root `pyproject.toml` contains a `[tool.uv.workspace]` table, callisto MUST treat that table as authoritative for Python workspace membership, following the same "declared workspace root wins" precedent `WorkspaceCargoResolver` establishes for `[workspace]` in `Cargo.toml`:
- `members`: an array of glob patterns (e.g. `["packages/*", "libs/*"]`) resolved relative to the directory containing the root `pyproject.toml`. Each glob match that itself contains a `pyproject.toml` is a workspace member.
- `exclude`: an array of glob patterns evaluated against the same candidate set; any directory matching `exclude` MUST be dropped from the member list even if it also matches `members`. This mirrors uv's own documented member/exclude precedence (exclude wins).
- A directory is only a genuine member if `<member_dir>/pyproject.toml` exists and parses; a glob match without a `pyproject.toml` at its root MUST be silently skipped, not treated as an error, since globs commonly match non-package directories.
- The workspace root itself (the directory holding `[tool.uv.workspace]`) is a member only if it also declares `[project]` (i.e. the root is itself an installable package, not just a workspace manifest). This parallels how a Cargo workspace root is only a package member when it carries its own `[package]` table.

Detection of the uv workspace root follows the same "walk upward from the discovery start point looking for a declaring manifest" strategy `find_workspace_root` (`callisto-graph::locate::root`) already uses for Cargo (`[workspace]` in `Cargo.toml`) and npm (`workspaces` in `package.json`, or `pnpm-workspace.yaml`): callisto MUST extend `is_workspace_root` to also return true for a directory whose `pyproject.toml` contains a `[tool.uv.workspace]` table, so a single canonical workspace-root resolution pass covers Cargo, npm, and Python together instead of running three independent root searches.

### 5.2. Poetry and Poetry-only monorepos (no native workspace concept)

Poetry, as of the versions in common use, has no `[tool.uv.workspace]`-equivalent construct. A Poetry-based monorepo conventionally expresses inter-package relationships through `path = "../sibling-package"` entries under `[tool.poetry.dependencies]` rather than through a declared member list. Callisto MUST NOT infer workspace membership from the mere presence of `path` dependencies, because that would conflate "this package depends on that package" (an edge, already handled by `iter_dependencies`) with "these packages are co-released in one workspace" (membership, a discovery-time concept) — the two are related but distinct, and collapsing them would make workspace membership depend on dependency-graph shape rather than an explicit declaration.

Instead, a `pyproject.toml` tree that has no `[tool.uv.workspace]` anywhere in its ancestry MUST fall back to the same generic, unscoped `IgnoreWalkLocator` directory-walk discovery already `[LIVE: IMPLEMENTED]` for every other ecosystem lacking an explicit workspace file (this is the same fallback Cargo projects without a `[workspace]` root and npm projects without a `workspaces` key already receive: `IgnoreWalkLocator` finds every manifest reachable from the workspace root, independent of any workspace-membership declaration). Concretely: every directory under the callisto workspace root (as resolved by `find_workspace_root`) containing a parseable `pyproject.toml` with a resolvable package name is a discovered `ProjectRoot { ecosystem: Ecosystem::Pypi, .. }`, whether or not it participates in any `[tool.poetry.dependencies]` path relationship with a sibling.

### 5.3. Precedence summary

1. If any `pyproject.toml` between the discovery start path and the filesystem root declares `[tool.uv.workspace]`, that manifest's directory is the Python workspace root and its `members`/`exclude` globs are authoritative for membership (Section 5.1).
2. Otherwise, Python packages are discovered via the generic `IgnoreWalkLocator` walk from the overall callisto workspace root (Section 5.2), exactly as Poetry-only, Flit-only, and Hatch-only repositories are today.
3. A repository MAY mix both: a uv-workspace subtree nested inside a larger walk-discovered tree is legal, and `[tool.uv.workspace]` scoping applies only within that subtree; packages outside it are still picked up by the generic walk.

---

## 6. PEP 508 Dependency Constraint Rewrite Rules — **STATUS: [PLANNED: SPECIFIED]**

`callisto-manifests::lib::round_trip(ecosystem, spec, target)` currently has arms for `Ecosystem::Cargo` (`cargo::round_trip`) and `Ecosystem::Npm` (`npm::round_trip`) but no arm for `Ecosystem::Pypi`, so it falls through to the `_ => None` catch-all today. This section specifies the `Ecosystem::Pypi` arm's rewrite behavior before it is implemented; it is `[PLANNED: SPECIFIED]`, not present in `callisto-manifests::python` yet.

### 6.1. Guiding philosophy (shared with cargo.rs and npm.rs)

`cargo::round_trip` and `npm::round_trip` both follow the same precision-preservation rule: when a linked in-workspace dependency bumps to a new version, the specifier is re-rendered at the same operator and the same numeric precision (major-only, major.minor, or major.minor.patch) as the original clause, rather than being replaced with an exact pin. A `^1.0` Cargo dependency bumped to `1.2.0` becomes `^1.2`, not `^1.2.0`; a `~1.2.3` npm dependency bumped becomes `~1.2.4` at full precision because the original was already full precision. Comma/space-separated multi-clause requirements are split, each clause is rewritten independently by its own operator, and the pieces are rejoined. Any clause containing a wildcard (`*`, `x`, `X`) or a hyphenated range causes the whole rewrite to abort (`return None`) rather than risk mangling a range the renderer cannot faithfully reproduce — callers treat `None` as "leave this specifier untouched, this package needs a manual review".

The `Ecosystem::Pypi` arm MUST follow this same philosophy: preserve the caller's original operator and precision, split multi-clause PEP 508 specifiers on `,` before rewriting each clause, and return `None` (leave untouched) for any clause callisto cannot safely re-render, rather than silently normalizing formatting or precision the original author chose deliberately.

### 6.2. PEP 508 `[project.dependencies]` / `[project.optional-dependencies]` clauses

These are read into `DepSpec::Range(VersionReq, String)` today by `PyprojectToml::iter_dependencies`, where the `String` is the raw requirement clause with extras and environment marker already stripped (see Section 3). The planned `pypi::round_trip` MUST:
- Recognize the standard PEP 440 comparison operators `==`, `>=`, `<=`, `>`, `<`, `!=`, `~=`, and the compatible-release operator `~=` specifically (PEP 440's caret-equivalent), each re-rendered against `target` at the original clause's numeric precision, exactly as `cargo::round_trip` does for `^`/`~`/`>=`/`=`.
- Split on `,` for multi-constraint specifiers such as `">=2.28.0,<3.0.0"`, rewriting only the clause(s) whose operator targets a floor/pin against the bumped package (typically the `>=`/`==`/`~=` clause) and leaving upper-bound `<`/`!=` clauses untouched unless the target version would violate them, in which case `round_trip` MUST return `None` so the caller surfaces this as a manual-review case rather than silently producing a self-contradictory range.
- Preserve extras (`[security]`) and environment markers (`; sys_platform == 'win32'`) verbatim, exactly as `PyprojectToml::update_dependency_spec` already does today when splicing a new specifier back into the array entry — `round_trip` only computes the new specifier string; splicing it back around the existing extras/marker text remains `update_dependency_spec`'s job and is unaffected by this section.
- Return `None` (no rewrite) for any clause using PEP 440 wildcard matching (`==2.28.*`) or the arbitrary-equality operator `===`, matching the same "don't risk mangling a range we can't faithfully reproduce" abort behavior `cargo::round_trip`/`npm::round_trip` already apply to wildcards and hyphen ranges.

### 6.3. Poetry `[tool.poetry.dependencies]` clauses

`PyprojectToml` currently reads Poetry-table entries into the same `DepSpec::Range(VersionReq, String)` representation used for PEP 508 clauses (see `iter_dependencies`, Poetry branch) — the raw Poetry version string (e.g. `"^2.28.0"`, `"~1.4"`, `"2.28.0"` bare, or the `version` key of an inline table like `{ version = "^2.28.0", extras = ["security"] }`) is stored verbatim as the `String` half of `DepSpec::Range`, without translating Poetry's caret/tilde syntax into PEP 440 operators. The planned rewrite rules MUST preserve this existing representation rather than introduce a parallel Poetry-specific `DepSpec` variant:
- Poetry's `^` (caret) and `~` (tilde) operators have different semantics from PEP 508/PEP 440's `~=` and are NOT PEP 508 syntax; they MUST be recognized and rewritten as their own operator class, re-rendered at the original numeric precision the same way `cargo::round_trip` already handles Cargo's own `^`/`~` operators (Poetry's caret and Cargo's caret share the same "compatible within the leftmost nonzero component" semantics, so the existing `render_at_precision` pattern in `cargo.rs` is the correct model to mirror, not `npm.rs`'s).
- Bare version strings with no operator prefix (Poetry treats a bare `"2.28.0"` as an exact pin equivalent to `==2.28.0`) MUST be rewritten as a bare version at the same precision, not have an operator introduced that was not in the original.
- When the dependency is expressed as an inline table (`{ version = "^2.28.0", extras = [...] }`), only the `version` key is subject to rewrite; `extras`, `optional`, `markers`, and any other inline-table keys MUST be left untouched, matching how `update_dependency_spec`'s existing Poetry inline-table branch already isolates the `version` key today.

---

## 7. PEP 735 Dependency Groups (`[dependency-groups]`) — **STATUS: [PLANNED: SPECIFIED]**

Section 1 lists PEP 735 among the standards this engine targets. As of this revision, `callisto-manifests::python` contains no code reading, writing, or round-tripping `[dependency-groups]` — `PyprojectToml::iter_dependencies` only inspects `[project].dependencies` and `[tool.poetry.dependencies]` (Section 3), and `update_dependency_spec` only splices into those same two locations. This section specifies the intended design so the claim in Section 1 has a concrete, implementable target rather than standing as an unqualified compliance assertion; none of the following is implemented today.

### 7.1. Schema shape

PEP 735 defines a top-level `[dependency-groups]` table (a sibling of `[project]`, not nested under it) mapping group names to arrays of entries, where each entry is either:
- A plain PEP 508 requirement string (identical grammar to `[project.dependencies]` entries), or
- An "include" object `{ include-group = "other-group-name" }` that pulls in another group's entries by reference.

Example:
```toml
[dependency-groups]
test = ["pytest>=8.0.0", "pytest-cov>=5.0.0"]
lint = ["ruff>=0.5.0"]
dev = [{include-group = "test"}, {include-group = "lint"}, "ipython"]
```

### 7.2. Planned read behavior

`PyprojectToml::iter_dependencies` MUST be extended to walk `[dependency-groups]` after the existing PEP 621 and Poetry passes. Each plain-string entry parses using the identical partitioning rule already specified in Section 3 (extras in `[...]`, environment marker after `;`, PEP 440 requirement range). Each entry MUST be tagged with a `DepKind` that identifies it as an optional/group dependency distinct from `DepKind::Runtime` (mirroring how `[project.optional-dependencies]` extras and Poetry's `[tool.poetry.group.<name>.dependencies]` tables already need group-scoped classification), and MUST carry the originating group name so callers can distinguish, e.g., a `test` group entry from a `dev` group entry. `include-group` entries MUST be resolved by recursively including the referenced group's own entries (with cycle detection — a group MUST NOT transitively include itself) rather than being surfaced as a literal dependency named `include-group`.

### 7.3. Planned write behavior

`update_dependency_spec` MUST be extended so that, when a package with a matching name appears in any `[dependency-groups]` array entry, the same in-place specifier rewrite already applied to `[project].dependencies` array entries (Section 3, preserving extras/markers, splicing only the version-range substring) applies to the matching `[dependency-groups]` entry, per group, using `toml_edit`'s array-of-strings/array-of-inline-tables mutation so comment and formatting decor on the group table is preserved exactly as it already is for `[project].dependencies` today.

### 7.4. Interaction with round_trip (Section 6)

Once implemented, `[dependency-groups]` entries are PEP 508 strings and MUST use the exact same `pypi::round_trip` rewrite rules specified in Section 6.2 — there is no separate rewrite grammar for dependency-group entries, only a different table location to splice the result back into.

### 7.5. Immediate documentation correction

Until Sections 7.1-7.4 are implemented, Section 1's PEP 735 bullet is marked `[PLANNED: SPECIFIED]` (see that section) and must not be read as an implemented-compliance claim. This correction is the minimum fix required regardless of when the full feature lands.

---

## 8. Python Publishing (PyPI) — **STATUS: [PLANNED: SPECIFIED]**

Callisto's `PublishTarget` enum (`callisto-model::ecosystem`) already declares `PublishTarget::Pypi { index: Option<String> }`, and `PyprojectToml::publish_targets` already returns `vec![PublishTarget::Pypi { index: None }]` for any publishable Python package (`[LIVE: IMPLEMENTED]`, `callisto-manifests::python`). What does not exist yet is the orchestration side: `callisto-graph::commands::publish::plan_publish` currently branches only on `PublishTarget::CratesIo` (populating `PublishPlan.rust_crates`) and `PublishTarget::Npm { .. }` (populating `PublishPlan.npm_main_packages` / `npm_platform_packages`); there is no `PublishTarget::Pypi` arm, no `PypiPublish` entry type on `PublishPlan`, and `PublishOrchestrator::execute` has no PyPI branch. This section specifies that missing orchestration; none of it is implemented today.

### 8.1. Why Python needs a two-step publish, unlike Cargo/npm

`cargo publish` and `npm publish` are each a single command that builds and uploads a package in one step. Python has no equivalent single command: the conventional, PyPA-documented flow is two independent steps run by two independent tools:
1. **Build**: `python -m build` (the PyPA-blessed frontend around PEP 517 build backends — `hatchling`, `setuptools`, `flit_core`, `poetry-core`, `maturin`, whichever `[build-system].build-backend` the package declares) produces an sdist (`.tar.gz`) and a wheel (`.whl`) into a `dist/` directory.
2. **Upload**: `twine upload dist/*` uploads the built artifacts to the target index.

Callisto's planned `PublishTarget::Pypi` orchestration MUST model this as two ordered subprocess invocations, not one, unlike the single-invocation `CratePublish`/`NpmPublish` model. This is a structural difference from `RegistryClient::publish`'s current single-call shape (Section 8.3 below on `PublishOrchestrator` addresses how that shape needs to accommodate a build step).

### 8.2. Planned `PublishPlan` shape

`PublishPlan` (`callisto-model::plan`) MUST gain a `pypi_packages: Vec<PypiPublish>` field alongside the existing `rust_crates`, `npm_platform_packages`, and `npm_main_packages` fields. `PypiPublish` MUST carry at minimum: `name`, `version`, `publish_to: RegistryKey` (using the same `RegistryKey("pypi")` value `PublishTarget::Pypi::registry_key()` already returns today), and the resolved `index: Option<String>` from the originating `PublishTarget::Pypi { index }` (`None` meaning the default public PyPI index; `Some(url)` meaning a custom index such as a private PyPI-compatible server or TestPyPI). `plan_publish` MUST gain a `publishes_pypi` check parallel to the existing `publishes_cargo`/`publishes_npm` checks (matching `PublishTarget::Pypi { .. }` in `pkg.publish_to`) and push a `PypiPublish` entry into `pypi_packages` when a release is detected, following the exact same topological-order/`is_release` gating already applied to `rust_crates` and `npm_main_packages`.

### 8.3. Planned execution model

`PublishOrchestrator::execute` MUST gain a loop over `plan.pypi_packages` parallel to its existing `rust_crates`/`npm_main_packages`/`npm_platform_packages` loops. Because PyPI publishing is inherently two subprocesses rather than one `RegistryClient::publish` call, the orchestrator's Python path MUST:
1. Invoke `python -m build --sdist --wheel --outdir dist/<pkg>/` for the package directory (build step), failing the release for that package (without retrying — a build failure is not a transient registry error) if the build process exits non-zero.
2. Invoke `twine upload` against the built artifacts (upload step), reusing the existing `publish_with_retry` rate-limit/backoff loop already used for crates.io and npm, since PyPI's upload API can return HTTP 429 the same way crates.io and npm's registries can.
3. Treat "already published" responses from PyPI (HTTP 400 with a "File already exists" body, which is PyPI's documented idempotent-conflict signal) as success, not failure, mirroring this repository's existing convention (see `fix(action): treat already-published crates as idempotent success during release publish`) of treating already-published as idempotent success rather than a hard error, so a partially-completed publish run can safely resume.

Both steps run as subprocess invocations through the existing `CommandRunner` abstraction (`callisto_model::CommandRunner`), consistent with how the rest of the publish pipeline avoids ad hoc `std::process::Command` calls in favor of the injectable runner used elsewhere in `callisto-graph`.

### 8.4. Planned authentication model

Callisto MUST NOT invent its own PyPI credential storage. It MUST assume one of two externally-configured auth mechanisms, selected by environment rather than by callisto-specific configuration:
- **API-token auth (default, CI-friendly today)**: `twine` reads `TWINE_USERNAME` (conventionally the literal string `__token__` for token auth) and `TWINE_PASSWORD` (the PyPI API token, `pypi-...`) from the environment, or falls back to the system keyring if neither is set. Callisto's role is limited to invoking `twine upload` with the target index URL (`--repository-url` when `index: Some(url)` is set) and leaving credential resolution entirely to twine's own environment/keyring lookup — callisto MUST NOT read, log, or pass tokens as CLI arguments (which would leak them into process listings), matching the general secret-handling posture already implied by this repository's registry-auth handling for crates.io/npm.
- **Trusted publishing / OIDC (preferred for CI, no long-lived secret to manage)**: PyPI's Trusted Publisher feature lets a CI environment (GitHub Actions, GitLab CI) exchange a short-lived OIDC identity token for a PyPI API token with no `TWINE_PASSWORD` stored as a repository secret at all. When running under a CI provider PyPI trusted publishing supports, callisto SHOULD prefer this path — concretely, detecting that no `TWINE_PASSWORD`/`TWINE_USERNAME` is set and that the invoking CI environment exposes the provider's OIDC token endpoint, and delegating the token exchange to the standard `id-token: write` / `pypa/gh-action-pypi-publish`-style flow rather than requiring the user to provision a static API token — and MUST fall back to the static-token path (`TWINE_PASSWORD`) when OIDC trusted publishing is not configured or not available in the current environment.

### 8.5. Mapping onto `PublishTarget::Pypi`

`PublishTarget::Pypi { index: Option<String> }`'s `index` field is the sole configuration surface this design requires from the manifest side: `None` publishes to the default public PyPI index with whichever auth mechanism (Section 8.4) is available in the environment; `Some(url)` publishes to that URL instead (e.g. `https://test.pypi.org/legacy/` for TestPyPI, or a private index), passed to `twine upload --repository-url`. No additional fields are required on `PublishTarget::Pypi` for the design in this section; auth mechanism selection (Section 8.4) is an environment-time decision, not a manifest-declared one, consistent with how crates.io/npm registry auth is already handled outside the manifest layer today.
