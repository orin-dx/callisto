---
name: bug-hunter
description: >-
  Universal, repo-agnostic Rust bug hunting framework. Finds silent failures, spec-vs-code drift, ordering/staleness bugs, crash-safety violations, and edge cases across any Rust codebase using Hazard-Taxonomy Partitioning.
---

# Universal Rust Bug-Hunter Skill

## Goal

Find real bugs before they ship in any Rust repository (monorepos, CLI engines, libraries, polyglot WASM plugins): silent failures, spec-vs-code drift, data corruption, ordering staleness, crash-safety violations, and edge cases that break production.

This skill is **Rigid**: follow it exactly across any Rust workspace.

## The Law

> NO FINDING MARKED `CONFIRMED` WITHOUT AN END-TO-END TRACE. NO FIX MARKED DONE WITHOUT A TEST THAT FAILED BEFORE THE FIX AND PASSES AFTER, RUN IN THIS SESSION.

## Core Rules

- **Verify by Tracing**: Don't trust a comment, docstring, passing test, or a prior audit's "FIXED" label. Trace the code execution path yourself.
- **Hazard-Taxonomy Partitioning**: When executing multi-agent bug hunts, DO NOT partition subagents by directory or crate names. ALWAYS partition subagents by **Hazard Taxonomy** across the entire workspace.
- **Strict Invariants**: Prioritize data integrity, crash-safety, error propagation, and security bugs over style or refactoring.
- **Read-Only Investigation**: Investigate and report findings first. Do not mutate files unless explicitly requested to apply fixes.

---

## The 6 Universal Rust Hazard Taxonomies

When auditing any Rust repository, scan all code paths against these 6 hazard categories:

### 1. Discarded Data & Unused CLI / Struct Parameters
- Function parameters or struct fields prefixed with leading underscores (e.g. `_opts`, `_config`, `_loaded`) where user inputs or configuration flags are silently ignored.
- CLI flags parsed into argument structs (e.g., Clap `#[derive(Args)]`) but never read or evaluated before executing the gated operation.
- Functions returning `Ok(())` or exit code `0` while silently skipping intended operations.

### 2. Ordering, Mutability & Fixpoint Staleness Bugs
- First-write-wins patterns (`.or_insert()`, `entry().or_default()`) used where max-value-wins, latest-write-wins, or fixpoint convergence is required.
- Graph traversals or solver loops that modify target state without re-enqueuing dependents into a worklist, leaving downstream dependencies stale.
- Loop iterations that leak transient state across runs without proper resets.

### 3. Spec-vs-Code Compliance Drift
- Trace stated requirements from design docs, specs, READMEs, or docstrings to the underlying Rust functions. Ensure invariants are enforced by logic, not just represented in types.
- Unhandled enum variants or missing conditional branches in pattern matches.

### 4. Silent Fallbacks & Falsified Defaults
- Catch-all fallbacks (`unwrap_or_else`, `unwrap_or_default`, fallback zero SHAs, placeholder strings) that hide missing or malformed data instead of returning an explicit `Result::Err`.
- Swallowed I/O or subprocess errors (`if cmd.is_ok() { ... }`, `let _ = atomic_write(...)`) returning success reports when disk operations fail.

### 5. Boundary Inputs & Format Edge Cases
- Text/Format handling: UTF-8 BOM (`\u{FEFF}`) prefixes, CRLF line endings, missing trailing newlines, non-ASCII Unicode strings.
- Workspace boundaries: empty workspaces, single-package targets, cyclic dependencies, detached HEAD git states, un-tagged repositories.

### 6. Crash-Safety & Subprocess Security
- File I/O: Missing `.flush()` or `.sync_all()` calls prior to atomic file rename/persists.
- Subprocess invocations (e.g., `git`, `cargo`, `npm`, `tar`): Misplaced `--` end-of-options delimiters, unescaped string parameters, or bad flag ordering (`fatal: too many arguments`).

---

## Hazard-Taxonomy Multi-Agent Dispatch Matrix

When conducting an automated bug hunt across a Rust repository, launch subagents partitioned by Hazard Category across the entire workspace:

| Agent Role | Target Taxonomies | Objective |
| :--- | :--- | :--- |
| **Agent A: Data & Parameters** | Taxonomies 1 & 4 | Audit all crates for unused CLI flags, discarded parameters, and `unwrap_or` fallback defaults. |
| **Agent B: Solvers & Logic** | Taxonomies 2 & 3 | Audit fixpoint loops, graph solvers, ordering staleness, and spec compliance drift. |
| **Agent C: System & I/O** | Taxonomies 5 & 6 | Audit UTF-8 BOM, CRLF, atomic file write `.flush()`/`.sync_all()`, and subprocess argument ordering. |

---

## Evaluation Output Standard

Report each confirmed finding in this standard technical format:

```markdown
### [Severity: Critical | High | Medium | Low] <Brief Vulnerability Title>

- **Status**: CONFIRMED (execution path fully traced, file:line cited) | PLAUSIBLE (strong signal, not fully traced)
- **Location**: `path/to/file.rs:L123-L135`
- **Classification**: [Discarded Parameter | Fixpoint Staleness | Spec Drift | Silent Fallback | Boundary Condition | I/O Safety]
- **Root Cause**: Concise explanation of the flaw in the current implementation logic.
- **Failing Scenario**: Concrete payload, CLI command, or input state that triggers the defect.
- **Verification Strategy**: A test that fails on current code and passes once fixed.
```

### Verification Checklist
Before marking a finding `CONFIRMED`:
- [ ] Traced exact execution path end-to-end.
- [ ] Created a concrete failing scenario.
- [ ] Verified finding is not a duplicate.

Before marking a fix `DONE`:
- [ ] Test failed pre-fix (red).
- [ ] Test passes post-fix (green).
