# Handoff: release-trust spec track

**Branch:** `codex-release-trust-specs` (based off `origin/main` at `59023bd3`, currently at commit `dfdee5cd`)
**Status:** spec-only. No production code has been implemented for any of these tracks. One doc-tooling spec (005) has a real implementation on a separate branch, one test short of passing verification.

This document is a factual handoff, not a defense of prior work. It states what's done, what's blocked, and exactly why, so work can continue without re-deriving context.

---

## What's actually done

**SPEC-DOCUMENTATION-AUTHORITY-005** — implemented on branch `codex/005-doc-authority-v2` (rebased onto this branch's `39b250ae`, has its own commits on top: `4c3e0112`..`23beeaa9`, plus `fa525ee9`). Real code: `docs/AUTHORITY.md`, three corrected stale-doc claims, `scripts/lint_docs.py` + `scripts/test_lint_docs.sh`, Justfile wiring into `ci`/`ci-fast`. `just lint-docs` passes clean against the real repo.

**One gap, precisely located:** a mutation-testing pass (deliberately breaking the code 8 ways, checking the tests catch it) found the test suite doesn't catch merging `DOCS_VALID_AUTHORITY` and `SEMANTIC_MODEL_VALID_AUTHORITY` together at `scripts/lint_docs.py:34-41` — the exact cross-glob-contamination bug AC-004 exists to prevent. Every other injected mutation was caught. **Fix: add one test asserting a docs/*.md file carrying `authority: verified-current` (a semantic-model-only value) is rejected, and vice versa.** This is the only known remaining gap on this spec.

---

## What's spec-only, and why it's stuck

Six other specs (`SPEC-RELEASE-PLAN-DURABILITY-001A/001B/001C`, `SPEC-RELEASE-SOURCE-PROVENANCE-002`, `SPEC-GITHUB-RELEASE-HARDENING-003`, `SPEC-SUPPLY-CHAIN-REPRODUCIBILITY-004`, `SPEC-RELEASE-LANE-POLICY-006`) exist in `.claude/specs/` but have never passed `canon:exit-gate` (an adversarial spec-quality check) after 5-6 rounds of revision each. The failures stopped being wording/precision issues several rounds ago and are now real architecture questions. Three are worth flagging explicitly because they're genuine bugs, not spec nitpicks:

### 1. Real dependency-cycle problem in 001A

`SPEC-RELEASE-PLAN-DURABILITY-001A` (envelope/digest scheme) places digest computation in `crates/callisto-model/src/envelope.rs`. But computing `configDigest`/`inputDigest` requires calling into `callisto-graph`'s config resolver (`crates/callisto-graph/src/config/resolve.rs`) and package walker (`crates/callisto-graph/src/locate/ignore_walk.rs`). `callisto-model` is a Layer-1 leaf crate with no `callisto-*` dependencies; `callisto-graph` *depends on* `callisto-model`. As specified, this is a cycle. **Needs a real decision**: either digest computation lives in `callisto-graph` and calls into `callisto-model`'s types, or `callisto-model` gains a narrower "pre-resolved input" type that `callisto-graph` populates and passes in. This is an architecture call, not a wording fix.

### 2. Real cross-build correctness bug in 001A

001A AC-003 sorts a package's `dependencies` array "by `name` then `kind`". `DepKind` (`crates/callisto-model/src/dependency.rs:9-17`) derives `Ord` in declaration order (Runtime, Dev, Peer, Optional, Build), but its camelCase serialization sorts lexicographically differently (build, dev, optional, peer, runtime). The spec never says which ordering to use for the tiebreak. Since the same dependency name can appear under two kinds, **two spec-conforming implementations would produce different `inputDigest` values for the same input** — which is exactly the cross-build divergence this whole digest scheme exists to prevent. Fix is mechanical (pick one ordering, state it explicitly) but the bug itself is real and would have shipped silently.

### 3. Real logic bug in 001B

001B adds `Planned`/`Attempted`/`Skipped` states to `PublishAttemptResult`. But `crates/callisto-graph/src/commands/publish.rs:723-729` defines publish success as `!a.result.is_failure()` — and `is_failure()` only matches `Failed`. Under the new spec, **an interrupted run's status record would make every never-attempted operation look like it succeeded**, and `callisto tag` (in scope for 001B) could tag and release packages that were never actually published. No criterion in the current draft covers this. Needs either a redefinition of what counts as "safe to tag" or an explicit terminal-vs-non-terminal distinction wired through the tag path.

### Smaller, still-open items (001C, 004, 006)

- **001C**: the plan-emit command's output-path argument is referenced but never defined anywhere in scope; the empty-plan case has contradictory handling between AC-001 and AC-002.
- **004** (supply-chain audit): AC-001 mandates a record field with no way to obtain it while forbidding the only means of obtaining it (needs re-reading against the current draft, likely a leftover from an earlier revision round); AC-005 pins 8 of 10 exception-record fields to literals but leaves 2 as underspecified predicates.
- **006** (lane policy): AC-001 states two mutually exclusive requirements about the same taxonomy (self-contradictory as currently worded); AC-002a/AC-003's rejection rule is a universal quantification that's unsatisfiable once more than one package qualifies (doesn't say which one the diagnostic names).

Full blocker text for all of these is in this session's transcript / the `canon:exit-gate` outputs — not reproduced here since it's revision-round-specific and some of it may already be stale against your read of the current files.

---

## Current spec dependency graph

```
001A (envelope/digest) ← 001B (CLI modes) ← 001C (GH Actions topology)
                       ↖ 002 (source provenance) ← 003 (GH hardening, also depends on 001B/001C)
004 (supply chain) — independent
005 (doc authority) — independent, DONE except the one test above
006 (lane policy) — independent
```

`.claude/specs/INDEX.md` establishes accepted specs as top-level normative requirements (no `linked_requirement`/`REQ-*` backing files exist repo-wide — that's an intentional, documented convention, not an oversight).

---

## Recommendation

005 is one test away from shippable — land that first. The other six need real design decisions on the three items above (especially the callisto-model/callisto-graph cycle and the tag-after-interrupted-publish bug) before another spec-wording pass is useful. Further automated `canon:exit-gate` iteration without resolving those design questions first will likely keep surfacing downstream consequences of the same unresolved architecture calls.
