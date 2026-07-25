# Callisto

Callisto is a polyglot monorepo versioning and release tool for Rust (`Cargo`), JavaScript/TypeScript (`npm`, `pnpm`, `yarn`), Python (`pyproject.toml`), Go (`go.mod`), Deno (`deno.json`), and Moon (`moon`).

---

## Workspace Architecture

Callisto is split into 10 workspace crates:

| Layer | Crate | License | Purpose |
| :--- | :--- | :--- | :--- |
| **Layer 1** | [`callisto-model`](crates/callisto-model) | MIT/Apache-2.0 | Domain types, version models, JSON report contracts |
| | [`callisto-format`](crates/callisto-format) | MIT/Apache-2.0 | Changeset `.md` and `pre.json` parsers |
| | [`callisto-conventional`](crates/callisto-conventional) | AGPL-3.0 | Conventional commit bump classification |
| | [`callisto-changelog`](crates/callisto-changelog) | AGPL-3.0 | Changelog markdown generator |
| **Layer 2** | [`callisto-manifests`](crates/callisto-manifests) | AGPL-3.0 | Format-preserving manifest AST editors and atomic disk writes |
| | [`callisto-vcs`](crates/callisto-vcs) | AGPL-3.0 | In-process Git operations powered by `gix` (gitoxide) |
| **Layer 3** | [`callisto-graph`](crates/callisto-graph) | AGPL-3.0 | Dependency DAG resolution and Tarjan SCC cycle diagnostics |
| **Layer 4** | [`callisto-cli`](crates/callisto-cli) | AGPL-3.0 | CLI binary, colored diff previews, and `miette` diagnostics |
| | [`callisto-moon`](crates/callisto-moon) | AGPL-3.0 | Moon extension protocol implementation (`extism-pdk`) |
| **Dev** | [`callisto-fixtures`](crates/callisto-fixtures) | AGPL-3.0 | Test corpus and in-memory test doubles |

---

## Key Features

- **Format Preservation**: Edits `Cargo.toml` and `package.json` without altering existing formatting, comments, key order, or tab/space indentation.
- **Atomic File Writes**: Replaces files using temporary directory swaps (`NamedTempFile` + `fs::rename`) to prevent partial writes.
- **Cycle Diagnostics**: Uses Tarjan's SCC algorithm to detect circular dependencies and report exact node paths.
- **Native Git Engine**: Uses `gix` for fast in-process commit walks and reference matching.
- **CLI Safety**: Includes `--dry-run` colored diff previews and pipe-safe terminal output (`anstream`).
- **Schema Export**: Generates draft-07 JSON Schemas for language servers (`callisto schema --type <name>`).
- **Moon Integration**: Compiles to WASM (`wasm32-wasip1`) for execution inside Moon's plugin sandbox.

---

## Why Callisto?

Callisto builds upon the ideas of `@changesets/cli` and `knope`, addressing key limitations in multi-language monorepos:

| Dimension | `@changesets/cli` | `knope` | Callisto |
| :--- | :--- | :--- | :--- |
| **Runtime** | Node.js | Native Rust | Native Rust (~10ms execution) |
| **Languages** | JavaScript / npm only | Cargo, npm | Polyglot (Cargo, npm, PyPI, Go, Deno) |
| **Git Engine** | External `git` CLI calls | External `git` CLI calls | In-process native Git via `gix` (gitoxide) |
| **Monorepo Integration**| JS workspaces only | Basic workspaces | Native Moon plugin (`callisto-moon`) + standalone CLI |
| **File Edits** | Re-formats JSON | Limited TOML edits | Format-preserving AST edits + atomic tempfile swaps |
| **Graph Solver** | Basic peer cascades | Manual cascades | `petgraph` DAG solver + Tarjan SCC cycle diagnostics |

---

## Usage

```bash
# Build binary
cargo build --release

# View workspace status
callisto status

# Create a changeset
callisto add --package my-crate:minor --summary "Add feature"

# Preview version bumps with colored diffs
callisto version --dry-run

# Export JSON Schema
callisto schema --type status > status-schema.json
```

---

## Testing

```bash
# Run tests
cargo test --all-targets

# Check lints
cargo clippy --all-targets -- -D warnings

# Check formatting
cargo fmt --check
```
