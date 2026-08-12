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

> **Sub-10ms Native Speed**  
> Native Rust binary executes workspace discovery and version planning in under 10ms — 30x to 50x faster than `@changesets/cli` or `release-please`.

> **Concrete Syntax Tree (CST) Format Preservation**  
> Edits `Cargo.toml` and `package.json` using `toml_edit` and `serde_json` indentation fingerprinting, preserving user comments, table ordering, and whitespace.

> **Crash-Safe Atomic Disk Writes**  
> Writes updates to `NamedTempFile` temporary buffers before atomic POSIX `fs::rename` swaps, preventing corrupt or partial manifest writes during process interruption.

> **Topological Directed Graph Solver**  
> Models workspace package dependencies using `petgraph`'s Kahn topological solver and Tarjan's SCC algorithm to compute cascading version bumps and catch circular dependency cycles.

> **Zero-Config Native Matrix Auto-Discovery**
> `callisto matrix` auto-discovers napi-rs and maturin native build targets (`napi.targets`, `[tool.maturin].targets`) plus npm/PyPI runtime-version constraints (`engines.node`, `requires-python`) directly from manifests as the single source of truth, eliminating duplicate CI YAML configuration drift. Java (`java.version`) and .NET native-AOT target discovery are planned for a future release.

> **Hermetic & Build-System Agnostic**  
> Pure Rust CLI engine runs seamlessly in Bazel sandboxes (`rules_callisto`), Buck2, Nix flakes, Moon WASM (`callisto-moon`), GitHub Actions, GitLab CI, and local VCS hooks (`just hooks`).

---

## Quick Start (4 Steps)

```text
$ callisto status
Status (schema v1):
  callisto-cli        0.1.0  ->  0.2.0  (minor)  [.changeset/swift-foxes-leap.md]
  callisto-graph      0.1.0  ->  0.1.1  (patch)  [cascade: callisto-cli]

$ callisto version
  + Updated 2 package manifests (CST preserved)
  + Appended changelog entries to CHANGELOG.md
  + Consumed 1 changeset (.changeset/swift-foxes-leap.md)
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
          publish: just publish
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
          CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}
          NPM_TOKEN: ${{ secrets.NPM_TOKEN }}
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

Callisto brings together the best ideas from `@changesets/cli`, Google's `release-please`, and `nx release` into a single, lightning-fast native Rust engine designed for modern polyglot monorepos.

### Sub-10ms Native Performance
- **Callisto**: Native Rust binary executes workspace discovery and dependency graph solving in under 10ms (30x-50x faster than JS tools).
- **Alternatives**: `@changesets/cli`, `release-please`, and `nx release` require heavy Node.js runtimes (300ms-500ms startup times).

### Safe, Format-Preserving Manifest Edits
- **Callisto**: Edits `Cargo.toml` and `package.json` using Concrete Syntax Tree (CST) AST editors (`toml_edit` and `serde_json` indentation fingerprinting), keeping comments, key order, and whitespace intact. File updates use POSIX crash-safe atomic tempfile swaps (`NamedTempFile` + `fs::rename`).
- **Alternatives**: `release-please` relies on fragile regular expressions. `@changesets/cli` re-serializes JSON with destructive default formatting.

### Topological Graph Solver & Cycle Diagnostics
- **Callisto**: Uses `petgraph`'s Kahn topological solver and Tarjan's SCC algorithm to calculate exact version bump cascades and catch circular dependency cycles with rich diagnostic cards (`miette`).
- **Alternatives**: `release-please` focuses on single-repo releases. `nx release` is coupled to Nx JavaScript trees.

### Zero-Config Native Matrix Auto-Discovery (`callisto matrix`)
- **Callisto**: Auto-discovers napi-rs and maturin native build targets plus npm/PyPI runtime-version constraints directly from manifests as the single source of truth, eliminating duplicate CI YAML configuration drift. Java and .NET native-AOT target discovery are planned for a future release.
- **Alternatives**: Requires manually maintaining 50-line matrix arrays in GitHub Actions YAML.

### Hermetic & Build-System Agnostic
- **Callisto**: Pure Rust CLI engine runs seamlessly in Bazel sandboxes (`rules_callisto`), Buck2, Nix flakes, Moon WASM (`callisto-moon`), GitHub Actions, GitLab CI, and local Git hooks (`just hooks`).
- **Alternatives**: Locked to GitHub REST APIs (`release-please`) or JavaScript workspace tools.

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
    plugin: 'https://github.com/orin-dx/callisto/releases/download/v0.1.0/callisto-moon.wasm'
```

---

## Workspace Crate Architecture

Callisto is structured into 10 workspace crates divided across permissive (`MIT OR Apache-2.0`) and copyleft (`AGPL-3.0-only`) licenses:

| Layer | Crate | License | Purpose |
| :--- | :--- | :--- | :--- |
| **Layer 1** | [`callisto-model`](crates/callisto-model) | MIT/Apache-2.0 | Domain primitives, version grammars, JSON report contracts |
| | [`callisto-format`](crates/callisto-format) | MIT/Apache-2.0 | Changeset `.md` and `pre.json` parsers and writers |
| | [`callisto-conventional`](crates/callisto-conventional) | AGPL-3.0 | Conventional commit parsing and bump severity classification |
| | [`callisto-changelog`](crates/callisto-changelog) | AGPL-3.0 | Markdown changelog renderer |
| **Layer 2** | [`callisto-manifests`](crates/callisto-manifests) | AGPL-3.0 | Format-preserving manifest AST editors and atomic file writes |
| | [`callisto-vcs`](crates/callisto-vcs) | AGPL-3.0 | Native in-process Git operations powered by `gix` (gitoxide) |
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
