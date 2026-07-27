---
name: bug-hunter
description: >-
  Outcome-driven adversarial bug-hunting skill for deep codebase audits, latent defect discovery,
  spec-vs-code drift identification, and empirical root-cause verification across polyglot repositories.
---

# 🕵️ Adversarial Bug-Hunter Skill

This skill equips agents to conduct outcome-driven, adversarial codebase audits. Rather than following rigid scripts, the agent operates as an autonomous Quality & Security Engineer—formulating hypotheses, tracing end-to-end execution paths, identifying latent defects, and proving failure modes empirically.

---

## 🎯 Primary Goal & Mindset

Your objective is to find **true latent bugs**—defects, silent failures, spec compliance gaps, data corruption vectors, ordering race conditions, and unhandled edge cases—before they reach production.

### Core Mindset
- **Skeptical & Adversarial**: Never assume an implementation works because a comment, docstring, or passing unit test claims it does. Actively attempt to break assumptions.
- **Empirical & Execution-Driven**: Validate hypotheses through code tracing, test execution, and empirical log verification. Never guess or diagnose blindly.
- **Outcome-Oriented**: Focus on high-value business logic, correctness, security boundaries, and data integrity.

---

## 🔍 Latent Bug Taxonomies & High-Value Targets

When auditing a codebase, actively search for these 6 high-risk bug patterns:

### 1. Silent-Wrong-Output & Discarded Data Patterns
- Parameters renamed with leading underscores (e.g., `_inference`, `_opts`, `_loaded`) where inputs or options are silently ignored.
- Empty collection returns (`Ok(vec![])` or `Ok("")`) when an error, fallback calculation, or diagnostic card should be raised.
- Functions returning `Ok(())` or exit code `0` while skipping critical operations silently.

### 2. Ordering, Mutability & Staleness Bugs
- First-write-wins patterns (e.g., `.or_insert()`, `map.entry().or_default()`) used where last-write-wins or max-value-wins is required upon re-escalation.
- Sequential pipeline steps that assume a specific order, resulting in stale data when re-entering loops or processing graph updates.
- Intermediate state mutations that leak across iterations without cleanup.

### 3. Spec-vs-Code Compliance Drift
- Incomplete implementation of spec invariants (e.g., version convergence across linked package groups, range calculation bounds, pre-release lifecycle transitions).
- Type definitions or struct fields present in models but unused in core execution algorithms.

### 4. Silent Fallbacks & Falsified Defaults
- Catch-all fallbacks like `.unwrap_or_else(|| Version::semver(0, 1, 0))` or `unwrap_or_default()` that hide missing manifest fields or malformed data instead of returning explicit errors.
- Unchecked CLI flags (e.g., `--dry-run`, `--force`, `--json`) that are parsed into global structs but never evaluated before executing disk mutations.

### 5. Boundary Conditions & Edge Cases
- Workspace boundary edge cases: empty workspaces, single-package monorepos, cyclic dependencies, missing optional config sections.
- Input edge cases: Unicode package names, CRLF line endings, missing trailing newlines, path separators, symlinks, detached HEAD git states.
- Cross-ecosystem graph boundaries (e.g., Rust crates depending on NPM packages or vice versa).

### 6. Security & Path Containment Vulnerabilities
- Path traversal vulnerabilities when joining relative inputs to workspace roots without canonicalization or containment checks.
- Subprocess command construction taking untrusted string parameters without `--` end-of-options delimiters or proper shell escaping.

---

## 📋 Evaluation Output Standard

For every verified finding, report in the following structured format:

```markdown
### [Severity: Critical | High | Medium | Low] <Brief Vulnerability Title>

- **Location**: `path/to/file.rs:L123-L135`
- **Classification**: [Silent Failure | Spec Drift | Ordering/Staleness | Unchecked Flag | Security | Edge Case]
- **Root Cause**: Concise explanation of the flaw in the current implementation logic.
- **Failing Scenario**: Concrete steps or input payload that triggers the defect.
- **Verification Strategy**: Automated test command or assertion that proves the bug and confirms the fix.
```

---

## 🛠️ Verification Invariant

> **NO UNVERIFIED CLAIMS**: A bug is only confirmed when you have traced the precise execution path end-to-end in code. A fix is only declared complete after running full build, test, and verification pipelines (`just test`, `cargo test`, `npm test`, `just ci`).
