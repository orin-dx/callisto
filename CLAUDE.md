# Callisto Codebase Guide for Claude & AI Agents

This repository follows the centralized AI agent guidelines documented in [`AGENTS.md`](AGENTS.md).

---

## Quick Reference

### Primary Task Runner Commands
- `just` / `just ci` (default): Full verification suite, including the 90% line-coverage gate, zizmor, workflow contracts, and release-workflow-behavior. Excludes actionlint and the binary-dependent release-PR/artifact-preflight CI checks.
- `just test` / `moon run :test`: Run all unit, integration, doctests, and E2E lifecycle test suites.
- `just lint` / `moon run :lint`: Run Clippy lints with `-D warnings`.
- `just fmt` / `moon run :format`: Format code automatically.

### Key Architectural Invariants
1. **Safe Rust Only**: `unsafe_code = "forbid"` across all crates.
2. **Format Preservation**: Manifest modifications use CST editors (`toml_edit`, `serde_json` indentation fingerprinting).
3. **Atomic File Persistence**: File edits use `callisto_model::atomic::atomic_write`.
4. **License Isolation**: MIT foundation crates (`callisto-model`, `callisto-format`, `callisto-vcs`) must never depend on FSL product crates. All other crates use canonical `FSL-1.1-MIT`.
5. **No Emojis in Documentation**: Keep docs clean, technical, scannable, and emoji-free.

### Build/test scoping
Full pipeline: `just ci`/`just test`/`just lint`. For scoped iteration, one direct `cargo test -p <crate>` / `cargo clippy -p <crate>` invocation -- not `moon run <project>:test`, whose per-project fan-out serializes on `target/`'s build lock (see `Justfile`'s `lint` comment; `moon`'s own `test`/`lint` tasks are literally `cargo test -p $project` / `cargo clippy -p $project` each).

### `Cargo.lock`
Never `git checkout -- Cargo.lock` as pre-commit cleanup -- thrashes the build cache, can turn `just ci` into a multi-hour run. A diff means investigate, not discard.

### Closing out a bug fix or duplication fix
A fix is not done when the reported instance is patched. Before closing out:
1. Grep for structurally identical code elsewhere in the workspace (same algorithm, different file/ecosystem/mode -- not just the same literal string). If a sibling copy has the same defect, it was there before you looked; it doesn't become in-scope later, it's in-scope now.
2. If N copies of the same logic exist, prefer unifying into one implementation (helper/generic/shared function) over patching each copy in place. Patching N copies leaves N places for the next bug to hide in one of them; unifying to N=1 removes the class of bug, not just the instance.
3. If a sibling instance is found but deliberately deferred (out of scope for this PR), record it explicitly -- a changeset note, a tracked follow-up, a code comment -- not silence. Silent deferral is how the same pattern gets "found" again in a future audit and counted as new.
4. When a new shared helper is added specifically to replace duplicated logic, migrate every existing call site in the same change, or explicitly track the ones left un-migrated. A helper nobody is required to call is not a fix, it's an additional option nobody has to take.
