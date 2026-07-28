---
name: bug-hunter-rust
description: >-
  Trigger this skill when the user asks to perform a bug hunt, code audit, spec verification, or defect search in a Rust codebase or monorepo workspace. Use when checking for discarded CLI arguments, unhandled parameters, silent unwrap_or fallback defaults, graph fixpoint staleness, UTF-8 BOM or CRLF line ending boundary issues, missing atomic disk write flush or sync_all calls, or subprocess parameter option ordering errors. Also activate when the user requests an adversarial audit across safe Rust codebases, Cargo workspace dependencies, or WASM plugins.
---

# Universal Rust Bug-Hunter Skill

<overview>
This skill provides a self-contained, outcome-driven framework for auditing Rust codebases across 6 universal hazard taxonomies. It dynamically adapts to any Rust workspace structure (standalone crates, Cargo monorepos, polyglot WASM plugins) and automatically detects available workspace test runners (`cargo nextest`, `cargo test`, `just test`, `moon run :test`).
</overview>

---

<framework_references>
Review shared framework standards prior to executing an audit:
- [General Debugging Laws](../../../shared/debugging-laws.md)
- [Evaluation Report Template](../../../shared/report-template.md)
</framework_references>

---

<hazard_taxonomies>

### 1. Discarded Data & Unused CLI / Struct Parameters
- **Search Heuristic**: `fn\s+\w+.*\b_[a-zA-Z0-9_]+:\s*` or `let _ =`
- **Pattern**: Function parameters or struct fields prefixed with leading underscores where user inputs or configuration flags are silently ignored. Clap CLI fields parsed into structs but never read before executing gated operations.

### 2. Ordering, Mutability & Fixpoint Staleness
- **Search Heuristic**: `\.or_insert\(|\.entry\(.*?\)\.or_default\(`
- **Pattern**: First-write-wins patterns used where max-value-wins or fixpoint convergence is required. Graph traversals or solver loops modifying target state without re-enqueuing dependents into a worklist.

### 3. Spec-vs-Code Compliance Drift
- **Search Heuristic**: `TODO|FIXME|unimplemented!|todo!`
- **Pattern**: Stated requirements in design docs, specs, or docstrings that are represented in type signatures but missing enforcement logic in functions. Unhandled enum variants in pattern matches.

### 4. Silent Fallbacks & Falsified Defaults
- **Search Heuristic**: `\.unwrap_or_else\(|\.unwrap_or_default\(|\.unwrap_or\(`
- **Pattern**: Catch-all fallbacks hiding missing or malformed data instead of returning explicit `Result::Err`. Swallowed I/O or subprocess errors returning success reports when disk operations fail.

### 5. Boundary Inputs & Format Edge Cases
- **Search Heuristic**: `split\(|lines\(|from_utf8`
- **Pattern**: UTF-8 BOM (`\u{FEFF}`) prefixes, CRLF line endings, missing trailing newlines, non-ASCII Unicode strings, empty workspaces, detached HEAD Git states.

### 6. Crash-Safety & Subprocess Security
- **Search Heuristic**: `Command::new\(|runner\.run\(|NamedTempFile|persist\(`
- **Pattern**: File I/O missing `.flush()` or `.sync_all()` calls prior to atomic file rename/persists. Subprocess invocations with misplaced `--` option delimiters or bad flag ordering (`fatal: too many arguments`).

</hazard_taxonomies>

---

<subagent_dispatch_matrix>

| Agent Role | Target Taxonomies | Delegation Scenario |
| :--- | :--- | :--- |
| **`bug-hunter-scanner-rust`** | Taxonomies 1 & 4 | Delegate to scan workspace for unused CLI flags, discarded parameters, and silent `unwrap_or` defaults. Returns structured candidate signals. |
| **`bug-hunter-adversary-rust`** | Taxonomies 2 & 3 | Delegate to trace execution paths end-to-end, disprove candidate signals, and check graph solver fixpoints and spec drift. Returns confirmed findings with failing scenarios. |
| **`bug-hunter-remediator-rust`** | Taxonomies 5 & 6 | Delegate to audit boundary inputs, atomic write `.flush()`/`.sync_all()`, write failing regression tests (red), apply fixes, and verify clean test suite execution (green). |

</subagent_dispatch_matrix>
