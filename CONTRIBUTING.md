# Contributing to Callisto

Guidelines for developing, testing, and contributing code to Callisto.

---

## Development Setup

1. **Prerequisites**:
   - Rust toolchain (`stable`)
   - `git`

2. **Build**:
   ```bash
   git clone https://github.com/orin-dx/callisto.git
   cd callisto
   cargo build
   ```

---

## Pull Request Verification

All pull requests must pass the following checks before merging:

1. **Tests**:
   ```bash
   cargo test --all-targets
   cargo test --doc
   ```

2. **Lints**:
   ```bash
   cargo clippy --all-targets -- -D warnings
   ```

3. **Formatting**:
   ```bash
   cargo fmt --check
   ```

---

## Code Invariants

- **Safe Rust Only**: `unsafe_code = "forbid"` is enforced across all workspace crates.
- **Atomic File Writes**: File writes must use `callisto_manifests::atomic::atomic_write` (`NamedTempFile` + `fs::rename`) to prevent partial writes.
- **Format Preservation**: Manifest AST edits must preserve existing key order, comments, and space/tab indentation.
- **Error Cards**: User-facing errors must derive `miette::Diagnostic` with actionable remediation tips.
