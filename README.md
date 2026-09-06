<p align="center">
  <img src="assets/callisto-logo.png" width="180" alt="Callisto Release Engine Logo" />
</p>

<p align="center">
  <b>The fast, crash-safe, polyglot monorepo release engine.</b><br />
  <i>Replaces Node.js runtimes, fragile regular expression edits, and duplicate CI matrix YAML with a single native Rust binary.</i>
</p>

<p align="center">
  <a href="https://github.com/orin-dx/callisto/actions/workflows/callisto-ci.yml"><img src="https://github.com/orin-dx/callisto/actions/workflows/callisto-ci.yml/badge.svg" alt="CI" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg" alt="License" /></a>
  <a href="crates/callisto-model"><img src="https://img.shields.io/badge/unsafe_code-forbid-success.svg" alt="Safety" /></a>
</p>

---

## Key Capabilities

> **Native Speed**  
> A native Rust binary — no Node.js runtime startup cost — for workspace discovery and version planning.

> **Concrete Syntax Tree (CST) Format Preservation**  
> Edits `Cargo.toml` and `package.json` using `toml_edit` and `serde_json` indentation fingerprinting, preserving user comments, table ordering, and whitespace.

> **Crash-Safe Atomic Disk Writes**  
> Writes updates to `NamedTempFile` temporary buffers before atomic POSIX `fs::rename` swaps, preventing corrupt or partial manifest writes during process interruption.

> **Topological Directed Graph Solver**  
> Models workspace package dependencies using `petgraph`'s Kahn topological solver and Tarjan's SCC algorithm to compute cascading version bumps and catch circular dependency cycles.

> **Zero-Config Native Matrix Auto-Discovery**  
> `callisto matrix` auto-discovers napi-rs and maturin build targets (`napi.targets`, `[tool.maturin].targets`) plus npm/PyPI runtime constraints (`engines.node`, `requires-python`) straight from manifests — no duplicate CI YAML to keep in sync.  
> Java (`java.version`) and .NET native-AOT discovery are planned for a future release.

> **Native napi/maturin Platform-Package Coordination**  
> One native crate compiling to N architecture-specific npm/PyPI packages plus one wrapper package that depends on all of them is a first-class case, not a workaround: callisto gates the wrapper's publish on every platform sibling actually succeeding, so `optionalDependencies` never point at a version that was never uploaded. No other changesets-family tool models this shape at all.

> **Hermetic & Build-System Agnostic**  
> Pure Rust CLI engine runs seamlessly in Bazel sandboxes (`rules_callisto`), Buck2, Nix flakes, Moon WASM (`callisto-moon`), GitHub Actions, GitLab CI, and local VCS hooks (`just hooks`).

---

## Quick Start (4 Steps)

```text
$ callisto status
Status (schema v1):
  callisto-cli 0.1.0 (pending: minor)
  callisto-graph 0.1.0 (pending: patch)

$ callisto version
Version Plan (schema v1):
  callisto-cli 0.1.0 → 0.2.0
  callisto-graph 0.1.0 → 0.1.1
```

### Step 1: Initialize Callisto in Your Workspace

Run `init` in your repository root to create `callisto.toml` and `.changeset/`:

```bash
callisto init --yes
```

### Step 2: Create a Changeset

When adding a feature, fix, or breaking change to a package:

```bash
callisto add --package my-crate:minor --summary "Add authentication middleware"
```

This generates a `.changeset/<random-id>.md` file in your repository.

### Step 3: Inspect Workspace Status

View pending changesets and calculated version bumps across your monorepo DAG:

```bash
callisto status
```

### Step 4: Preview & Apply Version Bumps

Preview calculated manifest modifications with unified colored diffs:

```bash
# Preview diffs without modifying files
callisto version --dry-run

# Apply version bumps, update changelogs, and consume changesets
callisto version
```

> [!TIP]
> Run `callisto version --dry-run` locally anytime to inspect calculated version bumps and changelog updates with colored diffs before committing.

---

## Publishing Packages

`callisto version` bumps and commits; publishing to a registry is a separate, explicit step, split into commands that each do one thing and pass JSON to the next:

```bash
# 1. Compute what's ready to publish (read-only, no network)
callisto plan-publish --format json > plan.json

# 2. Publish each package to its registry (cargo/npm/twine), independently —
#    one package's rejection doesn't block the others
callisto publish --format json > report.json

# 3. Narrow the plan down to what the report confirms actually succeeded —
#    so one package's failure doesn't cost its already-shipped siblings a tag
callisto filter-plan --plan plan.json --report report.json > shipped.json

# 4. Tag the commits that shipped, moving any floating major alias (e.g. v1)
callisto tag --plan shipped.json --floating-major
```

```mermaid
sequenceDiagram
  participant CI as CI / callisto-action
  participant CLI as callisto CLI
  participant Reg as Registries
  participant Git as git remote

  CI->>CLI: plan-publish --format json
  CLI-->>CI: PublishPlan
  CI->>CLI: publish --format json
  CLI->>Reg: publish per package (cargo / npm / twine)
  Reg-->>CLI: per-package outcome
  CLI-->>CI: PublishReport
  CI->>CLI: filter-plan --plan --report
  CLI-->>CI: plan narrowed to confirmed successes
  CI->>CLI: tag --plan --floating-major
  CLI->>Git: create tags, move floating alias
```

`callisto-action` (the bundled GitHub Action) now creates or updates only the version PR. The repository release workflow performs plan/build/attested execute after that PR merges; see [`docs/06-publishing.md`](docs/06-publishing.md).

---

## Release Workflow & Branch Configuration

Callisto integrates natively with GitHub Actions, GitLab CI, and custom release pipelines.

### 1. Release PR Branch Naming & Configuration

When Callisto generates automated release pull requests, it targets the default release branch **`changeset-release/main`** (matching `@changesets/action` standards).

You can override the release branch name per invocation using the `--branch` flag:

```bash
# Generate PR body targeting a custom release branch
callisto compose-pr-body --branch release-packages
```

### 2. Production GitHub Actions Workflow (`release.yml`)

Create `.github/workflows/release.yml` to automate version bumps and registry publishing on `push` to `main`:

```yaml
name: Release

on:
  push:
    branches: [main]

concurrency:
  group: ${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: true

jobs:
  release:
    name: Release / Version Packages
    runs-on: ubuntu-latest
    permissions:
      contents: write
      pull-requests: write
      id-token: write  # OIDC registry publishing

    steps:
      - name: Checkout Repository
        uses: actions/checkout@v4
        with:
          fetch-depth: 0

      - name: Install Rust Toolchain
        uses: dtolnay/rust-toolchain@stable

      - name: Run Callisto Release Action
        uses: orin-dx/callisto-action@v1
        with:
          branch: changeset-release/main
          # The release workflow performs publication after merge.
```

### 3. Bypassing Heavy CI Workflows on Release PRs

Release PRs contain automated version bumps and changelog updates. To prevent running expensive CI build matrices on release PRs, ignore `changeset-release/**` branches in your main `.github/workflows/ci.yml`:

```yaml
name: CI Pipeline

on:
  push:
    branches: [main]
  pull_request:
    branches-ignore:
      - 'changeset-release/**'
```

---

## Why Callisto?

Callisto combines ideas from `@changesets/cli`, `release-please`, and `nx release` into one native Rust engine built for polyglot monorepos:

| | Callisto | Alternatives |
| :--- | :--- | :--- |
| **Speed** | Native Rust binary — no runtime startup cost | `@changesets/cli`, `release-please`, `nx release` pay a Node.js startup cost |
| **Manifest edits** | CST-based (`toml_edit`, `serde_json`), preserves comments/order/whitespace | `release-please` uses regex; `@changesets/cli` re-serializes JSON with default formatting |
| **Cycle detection** | Kahn + Tarjan SCC with `miette` diagnostic cards | `release-please` is single-repo only; `nx release` is tied to Nx JS trees |
| **Matrix discovery** | Auto-discovers napi-rs/maturin targets and npm/PyPI runtime constraints from manifests (Java/.NET planned) | Manual 50-line matrix arrays in CI YAML |
| **Portability** | Runs in Bazel, Buck2, Nix, Moon WASM, GitHub Actions, GitLab CI, and local Git hooks | Locked to GitHub REST APIs or JS workspace tooling |

### Feature comparison

| Capability | Callisto | `@changesets/cli` | `release-please` | `knope` |
| :--- | :--- | :--- | :--- | :--- |
| Intent format | Changesets (byte-compatible) | Changesets | Conventional Commits | Changesets or commits |
| Cargo workspaces | Native | — | Plugin | Yes |
| npm workspaces | Yes | Yes | Yes | Yes |
| Cross-ecosystem cascade | Yes | — | Per-ecosystem | — |
| napi/maturin platform-package coordination | Native | — | — | — |
| GitHub Release binary assets | Planned | — | Yes | — |

*Cross-ecosystem cascade*: a version bump propagates along real dependency edges — a Cargo crate bump cascades into the npm packages that depend on it, automatically. *Platform-package coordination* is the sharper case: one native crate compiling to N architecture-specific npm/PyPI packages plus one wrapper package depending on all of them — nothing else in this table treats that shape as a first-class case instead of a hand-rolled CI workaround.

---

## Installation

> [!IMPORTANT]
> Callisto enforces safe Rust (`#![forbid(unsafe_code)]`) and POSIX atomic disk writes (`NamedTempFile` + `fs::rename`) across all workspace manifest edits.

### 1. Standalone Native Binary (Cargo)

```bash
cargo install callisto-cli
```

### 2. Pre-Built Release Binary (GitHub Releases)

Download pre-compiled binaries for Linux (x86_64) or macOS from [GitHub Releases](https://github.com/orin-dx/callisto/releases):

```bash
curl -sL https://github.com/orin-dx/callisto/releases/latest/download/callisto-linux-amd64.tar.gz | tar -xz -C /usr/local/bin
```

### 3. Moon Extension Plugin (WebAssembly)

Add Callisto as a WASM plugin in your repository's `.moon/workspace.yml`:

```yaml
extensions:
  callisto:
    plugin: 'https://github.com/orin-dx/callisto/releases/latest/download/callisto-moon.wasm'
```

---

## Workspace Crate Architecture

Callisto is structured into 10 workspace crates divided across permissive (`MIT OR Apache-2.0`) and copyleft (`AGPL-3.0-only`) licenses:

```mermaid
graph TB
  subgraph L1["Layer 1 — domain types"]
    direction LR
    model["callisto-model"]
    format["callisto-format"]
    conventional["callisto-conventional"]
    changelog["callisto-changelog"]
  end
  subgraph L2["Layer 2 — I/O"]
    direction LR
    manifests["callisto-manifests"]
    vcs["callisto-vcs"]
  end
  subgraph L3["Layer 3 — engine"]
    graph_["callisto-graph"]
  end
  subgraph L4["Layer 4 — surface"]
    direction LR
    cli["callisto-cli"]
    moon["callisto-moon"]
  end
  model --> graph_
  format --> graph_
  conventional --> graph_
  changelog --> graph_
  manifests --> graph_
  vcs --> graph_
  graph_ --> cli
  graph_ --> moon
```

| Layer | Crate | License | Purpose |
| :--- | :--- | :--- | :--- |
| **Layer 1** | [`callisto-model`](crates/callisto-model) | MIT/Apache-2.0 | Domain primitives, version grammars, JSON report contracts |
| | [`callisto-format`](crates/callisto-format) | MIT/Apache-2.0 | Changeset `.md` and `pre.json` parsers and writers |
| | [`callisto-conventional`](crates/callisto-conventional) | AGPL-3.0 | Conventional commit parsing and bump severity classification |
| | [`callisto-changelog`](crates/callisto-changelog) | AGPL-3.0 | Markdown changelog renderer |
| **Layer 2** | [`callisto-manifests`](crates/callisto-manifests) | AGPL-3.0 | Format-preserving manifest AST editors and atomic file writes |
| | [`callisto-vcs`](crates/callisto-vcs) | MIT/Apache-2.0 | Native in-process Git operations powered by `gix` (gitoxide) |
| **Layer 3** | [`callisto-graph`](crates/callisto-graph) | AGPL-3.0 | Dependency DAG solver and Tarjan SCC cycle diagnostics |
| **Layer 4** | [`callisto-cli`](crates/callisto-cli) | AGPL-3.0 | Standalone CLI binary, colored diff previews, `miette` diagnostic cards |
| | [`callisto-moon`](crates/callisto-moon) | AGPL-3.0 | Moon extension protocol implementation (`extism-pdk`) |
| **Dev** | [`callisto-fixtures`](crates/callisto-fixtures) | AGPL-3.0 | Multi-ecosystem corpus and in-memory test doubles |

---

## Development & Task Runners

Callisto uses `just` as its primary developer command runner, delegating workspace tasks to `moon`:

```bash
# Run full local CI suite (formatting, clippy lints, test suite, security audit, WASM check)
just ci

# Run test suite
just test

# Check clippy lints
just lint

# Check code formatting compliance
just fmt-check

# Format code automatically
just fmt

# Check security advisories
just audit
```
