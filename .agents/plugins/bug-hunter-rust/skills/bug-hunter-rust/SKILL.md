---
name: bug-hunter-rust
description: >-
  Trigger this skill when the user asks to perform a bug hunt, code audit, spec verification, architecture smell analysis, or defect search in a Rust codebase or monorepo workspace. Use when checking for discarded CLI arguments, unhandled parameters, silent unwrap_or fallback defaults, graph fixpoint staleness, UTF-8 BOM or CRLF line ending boundary issues, missing atomic disk write flush or sync_all calls, raw string identity mismatches, or lossy CST mutations. Also activate when conducting multi-agent adversarial audits across safe Rust codebases, Cargo workspace dependencies, or WASM PDK plugins.
---

# Universal Rust Bug-Hunter Skill

<overview>
This skill provides a self-contained, outcome-driven framework for auditing Rust codebases across 6 universal hazard taxonomies and 6 architectural code smells. It dynamically adapts to any Rust workspace structure (standalone crates, Cargo monorepos, polyglot WASM plugins), synthesizes recurring defects into reusable shared Rust traits, and coordinates 5 specialized subagents.
</overview>

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

<architectural_smell_sweeps>

### 1. Raw String Identity Smell
- **Symptom**: Comparing `PackageId` using `==` on raw `.name()` or `.to_string()` without prefix resolution.
- **Sweep**: `grep_search` for `\.name\(\)\s*==` or `pkg\.id ==`.

### 2. Un-Transactional Disk Mutation Smell
- **Symptom**: Mutating files or Git state step-by-step in non-dry-run paths without rollback transactions.
- **Sweep**: `grep_search` for `fs::write` or `fs::remove_file` in version plan resolution.

### 3. Lossy Serde / CST Formatting Smell
- **Symptom**: Replacing `toml_edit::Value` or `serde_json::Value` without preserving `.decor()` or line endings.
- **Sweep**: `grep_search` for `Value::from\(` or `insert\("version"`.

### 4. Un-Fsynced Directory Metadata Smell
- **Symptom**: `create_dir_all` or `atomic_write` without calling `.sync_all()` on parent directory handles post-rename.
- **Sweep**: `grep_search` for `atomic_write` or `fs::create_dir_all`.

### 5. Scopeless Fallback & Unbounded Traversal Smell
- **Symptom**: `unwrap_or_else` defaults hiding invalid state or `revwalk` traversing entire Git history.
- **Sweep**: `grep_search` for `rev_walk` or `unwrap_or_else`.

### 6. Hardcoded Constant Dummy Smell
- **Symptom**: Inserting placeholder strings (`"Release update"`, `"changeset.md"`) instead of preserving user metadata.
- **Sweep**: `grep_search` for `"Release update"` or `"changeset.md"`.

</architectural_smell_sweeps>

---

<shared_traits_centralization_framework>

When auditing multi-crate workspaces, map recurring defects directly to 7 Centralized Rust Traits:
1. `PackageIdentityResolver`: Cross-ecosystem package matching and bare name resolution.
2. `VersionSpecRenderer`: Format & precision-preserving version requirement rendering.
3. `ChangesetStorage`: Crash-safe, transactional disk engine with parent directory fsync.
4. `CstManifestEditor`: CST format-preserving manifest modification (`toml_edit` and `serde_json`).
5. `GitVcsProvider`: Safe Git tags, shallow checkout handling, and bounded revwalk.
6. `CascadeSolver`: Fixpoint dependency graph solver & convergence tracking.
7. `ReportPresenter`: Unified text & JSON report renderer with rich `miette` diagnostic cards.

</shared_traits_centralization_framework>

---

<subagent_dispatch_matrix>

| Agent Role | Target Taxonomies / Scope | Delegation Scenario |
| :--- | :--- | :--- |
| **`bug-hunter-scanner-rust`** | Taxonomies 1 & 4 | Delegate to scan workspace for unused CLI flags, discarded parameters, and silent `unwrap_or` defaults. Returns candidate defect signals. |
| **`bug-hunter-adversary-rust`** | Taxonomies 2 & 3 | Delegate to trace execution paths end-to-end, disprove candidate signals, and check graph solver fixpoints and spec drift. Returns confirmed findings. |
| **`bug-hunter-remediator-rust`** | Taxonomies 5 & 6 | Delegate to audit boundary inputs, atomic write `.flush()`/`.sync_all()`, write failing regression tests (red), apply fixes, and verify clean test suite execution (green). |
| **`bug-hunter-architect-rust`** | Smells 1-6 & Trait Centralization | Delegate to analyze recurring defect patterns, cluster code smells, and design lean Rust traits with formal contracts. |
| **`bug-hunter-mutator-rust`** | Mutation Testing & Test Coverage | Delegate to run `cargo-mutants`, find survived mutant branches, and write boundary unit tests ensuring assertions fail when code is mutated. |

</subagent_dispatch_matrix>
