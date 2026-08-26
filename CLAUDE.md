# Callisto Codebase Guide for Claude & AI Agents

This repository follows the centralized AI agent guidelines documented in [`AGENTS.md`](AGENTS.md).

---

## Quick Reference

### Primary Task Runner Commands
- `just ci`: Run full verification suite (Formatting, Clippy lints, Moon unit/integration tests, WASM target check, coverage). Matches CI parity; slow (coverage recompiles the whole workspace under instrumentation).
- `just ci-fast`: Same as `just ci` minus `coverage`. Use this for everyday pre-PR checks.
- `just test` / `moon run :test`: Run all unit, integration, doctests, and E2E lifecycle test suites.
- `just lint` / `moon run :lint`: Run Clippy lints with `-D warnings`.
- `just fmt` / `moon run :format`: Format code automatically.

### Key Architectural Invariants
1. **Safe Rust Only**: `unsafe_code = "forbid"` across all crates.
2. **Format Preservation**: Manifest modifications use CST editors (`toml_edit`, `serde_json` indentation fingerprinting).
3. **Atomic File Persistence**: File edits use `callisto_model::atomic::atomic_write`.
4. **Layer Isolation**: Layer 1 crates (`callisto-model`, `callisto-format`) must remain permissive (`MIT OR Apache-2.0`) and never depend on AGPL crates.
5. **No Emojis in Documentation**: Keep docs clean, technical, scannable, and emoji-free.

### Build/test scoping
Full pipeline: `just ci`/`just test`/`just lint`. For scoped iteration, one direct `cargo test -p <crate>` / `cargo clippy -p <crate>` invocation -- not `moon run <project>:test`, whose per-project fan-out serializes on `target/`'s build lock (see `Justfile`'s `lint` comment; `moon`'s own `test`/`lint` tasks are literally `cargo test -p $project` / `cargo clippy -p $project` each).

### `Cargo.lock`
Never `git checkout -- Cargo.lock` as pre-commit cleanup -- thrashes the build cache, can turn `just ci` into a multi-hour run. A diff means investigate, not discard.
