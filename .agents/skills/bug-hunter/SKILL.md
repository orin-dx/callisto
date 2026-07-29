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
- **Hazard-Taxonomy Partitioning**: When executing multi-agent bug hunts, DO NOT partition subagents by directory or crate names. ALWAYS partition subagents by **Hazard Taxonomy** or **Code Smell Sweep** across the entire workspace.
- **Strict Invariants**: Prioritize data integrity, crash-safety, error propagation, and security bugs over style or refactoring.
- **Read-Only Investigation**: Investigate and report findings first. Do not mutate files unless explicitly requested to apply fixes.

---

## The 6 Universal Rust Hazard Taxonomies

1. **Discarded Data & Unused CLI / Struct Parameters**: Leading-underscore parameters (`_opts`, `_config`), unused Clap CLI flags.
2. **Ordering, Mutability & Fixpoint Staleness**: First-write-wins vs max-value-wins, graph solver loops missing re-enqueueing logic.
3. **Spec-vs-Code Compliance Drift**: Requirements in design docs/specs missing enforcement logic in functions.
4. **Silent Fallbacks & Falsified Defaults**: Catch-all `unwrap_or` defaults hiding invalid state, swallowed I/O errors.
5. **Boundary Inputs & Format Edge Cases**: UTF-8 BOM (`\u{FEFF}`), CRLF line endings, unicode paths, detached HEAD Git states.
6. **Crash-Safety & Subprocess Security**: Atomic writes missing `.flush()` or `.sync_all()`, subprocess argument ordering.

---

## The 6 Architectural Code Smells & Ripgrep Sweeps

1. **Raw String Identity Smell**: `entry.name == pkg.id.to_string()` (Mismatched bare vs prefixed string comparison).
2. **Un-Transactional Disk Mutation Smell**: Step-by-step file edits without atomic batch rollback.
3. **Lossy Serde / CST Formatting Smell**: Replacing TOML/JSON nodes without preserving `.decor()` or line endings.
4. **Un-Fsynced Directory Metadata Smell**: `create_dir_all` or `atomic_write` missing parent directory `sync_all()`.
5. **Scopeless Fallback & Unbounded Traversal Smell**: Unbounded `revwalk` commit loops or catch-all `unwrap_or_else` defaults.
6. **Hardcoded Constant Dummy Smell**: Inserting placeholder strings (`"Release update"`, `"changeset.md"`) masking user metadata.

---

## 7 Shared Trait Centralization Framework

When auditing multi-crate workspaces, map recurring defects directly to 7 Centralized Rust Traits:
1. `PackageIdentityResolver`: Cross-ecosystem package matching and bare name resolution.
2. `VersionSpecRenderer`: Format & precision-preserving version requirement rendering.
3. `ChangesetStorage`: Crash-safe, transactional disk engine with parent directory fsync.
4. `CstManifestEditor`: CST format-preserving manifest modification (`toml_edit` and `serde_json`).
5. `GitVcsProvider`: Safe Git tags, shallow checkout handling, and bounded revwalk.
6. `CascadeSolver`: Fixpoint dependency graph solver & convergence tracking.
7. `ReportPresenter`: Unified text & JSON report renderer with rich `miette` diagnostic cards.

---

## Hazard-Taxonomy Multi-Agent Dispatch Matrix

| Agent Role | Scope | Objective |
| :--- | :--- | :--- |
| **`bug-hunter-scanner-rust`** | Taxonomies 1 & 4 | Scan workspace for unused CLI flags, discarded parameters, and silent `unwrap_or` defaults. |
| **`bug-hunter-adversary-rust`** | Taxonomies 2 & 3 | Trace execution paths end-to-end, disprove candidate signals, and check graph solver fixpoints and spec drift. |
| **`bug-hunter-remediator-rust`** | Taxonomies 5 & 6 | Audit boundary inputs, atomic write `.flush()`/`.sync_all()`, write failing regression tests (red), apply fixes. |
| **`bug-hunter-architect-rust`** | Smells 1-6 & Trait Centralization | Analyze recurring defect patterns, cluster code smells, and design lean Rust traits with formal contracts. |
| **`bug-hunter-mutator-rust`** | Mutation Testing & Coverage | Run `cargo-mutants`, find survived mutant branches, and write boundary unit tests ensuring assertions fail when code is mutated. |

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
