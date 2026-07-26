# Contributing to Callisto

Guidelines, engineering standards, testing expectations, and contribution workflows for Callisto.

---

## 1. Development Setup & Prerequisites

### Prerequisites

- **Rust Toolchain**: `stable` channel (managed via `rustup`).
- **WebAssembly Target**: `wasm32-wasip1` (for `callisto-moon` PDK plugin verification).
- **Moon / Proto** (Recommended): Primary monorepo task runner.
- **Just** (Recommended): Command runner for quick workspace recipes.

### Local Initial Build

```bash
# Clone the repository
git clone https://github.com/orin-dx/callisto.git
cd callisto

# Add WASM cross-compilation target
rustup target add wasm32-wasip1

# Run full local CI pipeline via Just & Moon
just ci
```

---

## 2. Primary Development Tasks (`just` & `moon`)

Callisto uses `moon` as its monorepo task engine and `just` as its developer command runner. All primary tasks are defined natively in `.moon/tasks/rust.yml` and wrapped by `justfile`:

| Action | Just Command (Primary) | Moon Command | Cargo Direct |
| :--- | :--- | :--- | :--- |
| **Run Full Local CI** | `just ci` | `moon run :format-check && moon run :lint && moon run :test` | `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test --all-targets` |
| **Run Tests** | `just test` | `moon run :test` | `cargo test --all-targets && cargo test --doc` |
| **Check Lints** | `just lint` | `moon run :lint` | `cargo clippy --all-targets -- -D warnings` |
| **Check Formatting** | `just fmt-check` | `moon run :format-check` | `cargo fmt --check` |
| **Format Code** | `just fmt` | `moon run :format` | `cargo fmt` |
| **Verify WASM Target** | `just wasm-check` | N/A | `cargo check -p callisto-moon --target wasm32-wasip1 --features pdk` |

---

## 3. Core Code Standards & Invariants

All contributions to Callisto must adhere to 5 strict engineering invariants:

### 1. Safe Rust Only (`unsafe_code = "forbid"`)
Callisto forbids `unsafe` code blocks across all 10 workspace crates. Memory safety and thread safety are guaranteed by the Rust compiler.

### 2. Format Preservation Guarantee
Callisto never uses regular expressions or line-based string manipulation to edit manifests (`Cargo.toml`, `package.json`).
- **TOML**: Edits must use `toml_edit` to parse and manipulate Concrete Syntax Trees (CST), preserving comments, table ordering, and whitespace.
- **JSON**: Edits must fingerprint indentation style (`IndentStyle::Tabs` vs `IndentStyle::Spaces(N)`) and preserve key insertion order using `serde_json`.

### 3. Crash-Safe Atomic Disk Writes
All manifest and configuration modifications must go through `callisto_manifests::atomic::atomic_write`. File writes create a `NamedTempFile` in the target file's parent directory, flush data to disk, and atomically replace the target file via `fs::rename`.

### 4. Direct & Actionable Diagnostics (`miette`)
Errors intended for CLI users must derive `miette::Diagnostic` with an explicit error code, clear diagnostic message, and actionable remediation tip.

### 5. Dual Licensing Tier Respect
- **Layer 1 Crates** (`callisto-model`, `callisto-format`): Dual-licensed under `MIT OR Apache-2.0`. Must NOT depend on AGPL-licensed crates (`callisto-graph`, `callisto-cli`, `callisto-manifests`).
- **Layer 2-4 Crates** (`callisto-graph`, `callisto-cli`, `callisto-moon`): Licensed under `AGPL-3.0-only`.

---

## 4. Interactive Changesets & CI Coverage

### Interactive Changeset Wizard

When adding a feature, fix, or breaking change, run `callisto add` in an interactive terminal to launch the 5-step wizard:

```bash
cargo run --bin callisto -- add
```

This interactive wizard:
1. Prompts for workspace package selection (MultiSelect).
2. Asks which selected packages require a **MAJOR** bump.
3. Asks which remaining packages require a **MINOR** bump (defaulting others to **PATCH**).
4. Prompts for the changeset summary text.
5. Displays a colored preview and requests confirmation before writing `.changeset/<human-slug>.md`.

For automated agent or script execution, pass explicit CLI flags:

```bash
cargo run --bin callisto -- add --package callisto-cli:minor --summary "Add interactive wizard"
```

### GitHub CI & Coverage Reports

GitHub Actions executes CI using `just` and `moon` (`moonrepo/setup-toolchain-action` and `extraactions/setup-just`), ensuring total parity between local developer environments and CI.

In addition, every pull request generates:
- **Test Summary Cards**: Published directly to GitHub Step Summaries.
- **Code Coverage Reports**: Generated via `taiki-e/cargo-llvm-cov-action@v1` and attached as `lcov.info` build artifacts.
