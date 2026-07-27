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
- **Skeptical & Adversarial**: Never assume an implementation works because a comment, docstring, unit test, or a prior audit's "FIXED"/"VERIFIED" label claims it does. Treat those labels as claims to falsify, not facts to build on — this repo has a track record of such labels being wrong. Actively attempt to break assumptions.
- **Empirical & Execution-Driven**: Validate hypotheses through code tracing, test execution, and empirical log verification. Never guess or diagnose blindly.
- **Outcome-Oriented**: Focus on high-value business logic, correctness, security boundaries, and data integrity.
- **Read-Only by default**: Investigate and report. Do not edit, fix, or run mutating commands unless the invoking task explicitly asks for fixes.
- **Ledger-Aware**: Before starting, read `FINDINGS.md` in this directory. Don't re-report a finding already logged there as `CONFIRMED` unless you have new evidence it's wrong or unfixed after being marked fixed. After a run, append any new `CONFIRMED` findings to it.

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

## 📋 Evaluation Output Standard

Before reporting a finding, spend one pass trying to *disprove* it — re-read the call site(s),
check for a guard/validation elsewhere in the path you may have missed. If it survives that
pass, report it. This catches plausible-but-wrong findings before they're logged.

For every finding, report in the following structured format:

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

## 🛠️ Verification Invariants (two separate claims — do not conflate them)

> **Bug confirmed** ≠ **fix complete**. A bug is `CONFIRMED` only once you've traced the precise
> execution path end-to-end in code. A fix is only ever reported as done after it has actually
> been applied *and* a real test run (`cargo test`, `just test`, `just ci`) has been observed to
> pass in this session — never asserted from memory, a comment, or another tool's prior claim.
