<p align="center">
  <img src="assets/callisto-logo.png" width="140" alt="Callisto Release Engine Logo" />
</p>

<h1 align="center">Contributing to Callisto</h1>

<p align="center">
  <b>Guidelines, engineering standards, testing expectations, and contribution workflows.</b>
</p>

By participating in this project, you agree to abide by the [Code of Conduct](./CODE_OF_CONDUCT.md). Found a security issue? See [SECURITY.md](./SECURITY.md) instead of opening a public issue.

---

## 1. Development Setup & Prerequisites

### Prerequisites

- **Rust Toolchain**: `stable` channel (managed via `rustup`).
- **WebAssembly Target**: `wasm32-wasip1` (for `callisto-moon` PDK plugin verification).
- **Moon / Proto** (Recommended): Primary monorepo task runner.
- **Just** (Recommended): Command runner for quick workspace recipes.

> [!NOTE]
> Install native local Git hooks with `just hooks` to run formatting checks automatically before every local commit (`pre-commit`) and push (`pre-push`).

### Local Initial Build

```bash
# Clone the repository
git clone https://github.com/orin-dx/callisto.git
cd callisto

# Add WASM cross-compilation target & install local Git hooks
rustup target add wasm32-wasip1
just hooks

# Run full local CI pipeline via Just & Moon
just ci
```

### Optional Entire session capture

Entire is an optional maintainer tool. The committed `.entire` configuration disables automatic
checkpoint pushes because sessions can include prompts, responses, tool calls, and file changes.
Do not push session data to this public repository by default.

If you use Entire, install and authenticate its CLI locally, then run `entire status --detailed`.
Use a separately approved private checkpoint remote before enabling session synchronization.
Agent-specific integrations are local generated files and are not required to contribute.

---

## 2. Primary Development Tasks (`just`)

Callisto uses `just` as its canonical developer command runner, delegating workspace tasks to `moon` under the hood:

| Action | Canonical Command | Description |
| :--- | :--- | :--- |
| **Run Full Verification CI** | `just ci` | Runs formatting, Clippy lints, unit/integration tests, audit, and WASM target checks |
| **Run Test Suite** | `just test` | Runs unit, integration, doctests, and E2E lifecycle test suites |
| **Check Clippy Lints** | `just lint` | Runs Clippy lints with `-D warnings` across all workspace crates |
| **Check Code Formatting** | `just fmt-check` | Verifies code formatting compliance |
| **Format Code** | `just fmt` | Applies `cargo fmt` formatting automatically |
| **Check Security Advisories** | `just audit` | Runs `cargo deny check advisories` security check |
| **Verify WASM Target** | `just wasm-check` | Verifies `wasm32-wasip1` cross-compilation target |

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

### 5. Licensing Tier Respect
- **MIT foundation crates** (`callisto-model`, `callisto-format`, `callisto-vcs`): Licensed under MIT and must not depend on FSL-licensed crates.
- **FSL product crates** (all remaining workspace crates): Licensed under canonical `FSL-1.1-MIT`, which grants an MIT license beginning two years after each release is first published.

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
