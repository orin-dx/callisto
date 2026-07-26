# Callisto

Callisto is a fast, polyglot monorepo versioning, changeset cascading, and release management tool written in Rust. It provides unified versioning, automated changelog generation, and atomic release orchestration across Rust (`Cargo.toml`), Node.js/npm (`package.json`), Python (`pyproject.toml`), Go (`go.mod`), Deno (`deno.json`), and Moon (`moon`).

---

## Key Design Guarantees

- **Sub-10ms Native Performance**: Written in Rust to execute workspace discovery, graph traversal, and version planning in `<10ms` (30x faster than `@changesets/cli`).
- **AST Format Preservation**: Edits manifests using concrete syntax tree (CST) editors (`toml_edit` and `serde_json` with custom indentation fingerprinting), keeping comments, key order, trailing commas, and whitespace intact.
- **Crash-Safe Atomic File Writes**: Writes updated manifests to temporary files (`NamedTempFile`) in the target directory and replaces targets via `fs::rename`, preventing partial or corrupt file writes if interrupted.
- **Graph Solver & Tarjan SCC Cycle Diagnostics**: Models workspace dependencies as a directed graph (`petgraph`). Automatically runs Tarjan's Strongly Connected Components (SCC) algorithm to detect circular dependencies before applying topological version cascades.
- **In-Process Native Git Engine**: Uses `gix` (gitoxide) for fast, thread-safe in-process repository discovery, ref matching, and commit walks without spawning `git` CLI subprocesses.
- **Moon Monorepo & WASM Plugin Integration**: Compiles to `wasm32-wasip1` for native execution inside Moon's Extism plugin sandbox (`callisto-moon`).

---

## Installation

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

## Quick Start (4 Steps)

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

---

## Complete Command Reference

| Command | Purpose | Example Usage |
| :--- | :--- | :--- |
| `callisto init` | Initializes `callisto.toml` and `.changeset/` directory | `callisto init --yes` |
| `callisto add` | Creates a new `.changeset/*.md` declaration file | `callisto add --package pkg-a:minor --summary "msg"` |
| `callisto status` | Reports pending changesets and workspace status | `callisto status --format json` |
| `callisto version` | Applies version bumps, updates changelogs, and consumes changesets | `callisto version --dry-run` |
| `callisto plan-publish` | Calculates publish ordering for unreleased packages | `callisto plan-publish --format json` |
| `callisto tag` | Creates Git release tags for published packages | `callisto tag --plan <plan_json>` |
| `callisto validate` | Validates changeset files in CI pull requests | `callisto validate` |
| `callisto schema` | Exports Draft-07 JSON Schemas for language servers | `callisto schema --type status` |
| `callisto pre` | Controls pre-release mode state (`enter` / `exit`) | `callisto pre enter beta` |

---

## Configuration Reference (`callisto.toml`)

Create a `callisto.toml` in your workspace root to configure package groups and changelog generation:

```toml
# Schema version
schema_version = 1

# Synchronize package versions in lock-step
[[fixed-group]]
name = "core-packages"
packages = ["callisto-model", "callisto-format", "callisto-graph"]

# Synchronize bump severity without forcing identical base versions
[[linked-group]]
name = "cli-tools"
packages = ["callisto-cli", "callisto-moon"]

[changelog]
commit_attribution = true
issue_links = true
```

---

## GitHub Actions Release Workflow Integration

Add `.github/workflows/callisto-release.yml` to automate Version Packages PR creation and release publishing:

```yaml
name: Release & Publish Workflow

on:
  push:
    branches: [main]

jobs:
  release:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0

      # Built-in Callisto Release Action:
      # - Creates "chore: version packages" PR when changesets exist
      # - Publishes packages & creates GitHub Releases when Version PR is merged
      - uses: orin-dx/callisto/actions/callisto-action@v1
        with:
          publish: 'cargo publish'
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
          CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}
```

---

## Why Callisto?

Callisto builds upon the ideas of `@changesets/cli` and `knope`, addressing key limitations in multi-language monorepos:

| Dimension | `@changesets/cli` | `knope` | Callisto |
| :--- | :--- | :--- | :--- |
| **Runtime** | Node.js (~300ms execution) | Native Rust | Native Rust (<10ms execution) |
| **Supported Ecosystems** | JavaScript / npm only | Cargo, npm | Polyglot (Cargo, npm, PyPI, Go, Deno) |
| **Git Engine** | External `git` CLI subprocesses | External `git` CLI subprocesses | In-process native Git via `gix` (gitoxide) |
| **Monorepo Engine Integration** | JS workspaces only | Basic workspaces | Native Moon WASM plugin (`callisto-moon`) + CLI |
| **Manifest Modification** | Destructive JSON re-formatting | Limited TOML edits | Format-preserving AST edits + atomic tempfile swaps |
| **Graph Resolution** | Basic peer cascades | Manual cascades | `petgraph` DAG solver + Tarjan SCC cycle diagnostics |

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

Callisto supports `just`, `moon`, and standard `cargo` workflows:

### Using `just` (Recommended Task Runner)

```bash
# Run full local CI suite (formatting, clippy lints, test suite, WASM check)
just ci

# Run unit, integration, doctests, and E2E tests
just test

# Check clippy lints with warnings treated as errors
just lint

# Check code formatting compliance
just fmt-check
```

### Using `moon` (Monorepo Runner)

```bash
# Run inherited tasks across workspace crates
moon run :test
moon run :lint
moon run :format-check
```

### Using `cargo` Directly

```bash
# Run unit and integration tests
cargo test --all-targets

# Run documentation doctests
cargo test --doc

# Run clippy lints
cargo clippy --all-targets -- -D warnings

# Check code formatting
cargo fmt --check
```
