---
name: bug-hunter
description: >-
  Finds silent failures, spec-vs-code drift, ordering/staleness bugs, and path/security issues by tracing execution end-to-end. Use for audit or adversarial bug-hunt requests.
---

# Adversarial Bug-Hunter Skill

## Goal

Find real bugs before they ship: silent failures, spec-vs-code drift, data corruption, ordering bugs, and edge cases that break production.

## Rules
- Don't trust a comment, docstring, passing test, or a prior audit's "FIXED"/"VERIFIED" label as proof something works — verify by tracing the code yourself.
- Verify by tracing code and running tests. Don't guess.
- Prioritize correctness, security, and data-integrity bugs over style.
- Read-only by default: investigate and report; don't edit files or run mutating commands unless the task explicitly asks for a fix.
- Read `FINDINGS.md` in this directory before starting. Don't re-report anything already logged there as `CONFIRMED` unless you have new evidence it's wrong, or it was marked fixed but isn't. Append new `CONFIRMED` findings when done.

---

## Bug Patterns to Search For

### 1. Silent-Wrong-Output & Discarded Data Patterns
- Parameters renamed with leading underscores (e.g., `_inference`, `_opts`, `_loaded`) where inputs or options are silently ignored.
- Empty collection returns (`Ok(vec![])` or `Ok("")`) when an error, fallback calculation, or diagnostic card should be raised.
- Functions returning `Ok(())` or exit code `0` while skipping critical operations silently.

### 2. Ordering, Mutability & Staleness Bugs
- First-write-wins patterns (e.g., `.or_insert()`, `map.entry().or_default()`) used where last-write-wins or max-value-wins is required upon re-escalation.
- Sequential pipeline steps that assume a specific order, resulting in stale data when re-entering loops or processing graph updates.
- Intermediate state mutations that leak across iterations without cleanup.

### 3. Spec-vs-Code Compliance Drift
- Incomplete implementation of a spec invariant — trace the full path from the spec's stated requirement to the code that's supposed to satisfy it; don't stop at a type definition or struct field that merely *represents* the invariant without a function that *enforces* it.
- Type definitions or struct fields present in models but unused in core execution algorithms.

### 4. Silent Fallbacks & Falsified Defaults
- Catch-all fallbacks (`unwrap_or_else`, `unwrap_or_default`, hardcoded placeholder values) that hide missing/malformed data instead of returning an explicit error.
- CLI flags or config options that are parsed into a struct but never read anywhere before the operation they're supposed to gate executes.

### 5. Boundary Conditions & Edge Cases
- Workspace boundary edge cases: empty workspaces, single-package monorepos, cyclic dependencies, missing optional config sections.
- Input edge cases: Unicode package names, CRLF line endings, missing trailing newlines, path separators, symlinks, detached HEAD git states.
- Cross-ecosystem graph boundaries (e.g., Rust crates depending on NPM packages or vice versa).

### 6. Security & Path Containment Vulnerabilities
- Path traversal vulnerabilities when joining relative inputs to workspace roots without canonicalization or containment checks.
- Subprocess command construction taking untrusted string parameters without `--` end-of-options delimiters or proper shell escaping.

---

## Evaluation Output Standard

Before reporting a finding, try to *disprove* it — re-read the call site(s) for a guard or validation you may have missed. If it survives that, report it.

Report each finding in this format:

```markdown
### [Severity: Critical | High | Medium | Low] <Brief Vulnerability Title>

- **Status**: CONFIRMED (execution path fully traced, file:line cited) | PLAUSIBLE (strong signal, not fully traced)
- **Location**: `path/to/file.rs:L123-L135`
- **Classification**: [Silent Failure | Spec Drift | Ordering/Staleness | Unchecked Flag | Security | Edge Case]
- **Root Cause**: Concise explanation of the flaw in the current implementation logic.
- **Failing Scenario**: Concrete steps or input payload that triggers the defect.
- **Verification Strategy**: Automated test command or assertion that proves the bug and confirms the fix.
```

Severity rubric:
- **Critical**: silent data loss/corruption, a security hole, or wrong output already reachable by a real user today.
- **High**: a spec'd feature is completely non-functional, but the failure is at least visible/discoverable (not silent).
- **Medium**: correctness/perf/robustness gap on a realistic but less common path (scale, edge-case input).
- **Low**: polish, DX, or a gap only reachable via an unrealistic input.

---

## Verification Invariants

> **Bug confirmed** ≠ **fix complete**. Mark `CONFIRMED` only once you've traced the execution path end-to-end in code. Mark a fix done only after it's applied and a real test run (`cargo test`, `just test`, `just ci`) has passed in this session — never from memory, a comment, or another tool's prior claim.
