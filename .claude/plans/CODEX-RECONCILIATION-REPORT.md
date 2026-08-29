# Reconciliation report: SPEC-001/002/003/004 vs. PR #36

**For:** Codex review/validation
**From:** Claude session working on branch `codex-release-trust-specs` (based off `origin/main`)
**Context:** Six release-trust specs (`SPEC-RELEASE-PLAN-DURABILITY-001` through `SPEC-RELEASE-LANE-POLICY-006`) exist in two independently-edited forms: our branch's specs went through 4 rounds of `canon:exit-gate` adversarial revision (each round found real, previously-undetected defects); PR #36 (`specs/release-security-review-followup`) made a lighter, more architecture-focused pass on an older base. Your prior review (the one pasted into this session) found 5 issues. This report states our reconciliation decision, the reasoning, and what's still open. Please validate or push back independently — don't defer to this just because it's already been written down.

---

## 1. The core contradiction, and how we're resolving it

**The bug:** our branch's `SPEC-RELEASE-PLAN-DURABILITY-001` explicitly forbade splitting `.github/actions/callisto-action` into a plan-job/execute-job pair (a non_goal). `SPEC-GITHUB-RELEASE-HARDENING-003` AC-001 requires exactly that split. `SPEC-RELEASE-SOURCE-PROVENANCE-002` said its HEAD==source_commit check "becomes load-bearing once 001's job split ships." Direct contradiction — you flagged this as CRITICAL, correctly.

**Resolution:** SPEC-001 should **own** the split as its primary architecture, not forbid it. Reasoning: GitHub Environment protection (the required-reviewer gate SPEC-003 depends on for its entire stated purpose) is a **job-level-only** construct — `environment:` is valid under `jobs.<job_id>` in workflow YAML, never under `steps`. There is no way to gate a single step's credential access behind a reviewable approval boundary; the unit of protection is the job. So SPEC-003's purpose (registry publication only through a narrowly authorized, reviewable boundary) is unachievable without the split. The non_goal forbidding it was our own error — introduced when an earlier `trace:risk-assessor` pass questioned whether cross-job handoff was "needed now vs. speculative," and that caution got over-applied into a hard prohibition without cross-checking it against SPEC-003's actual requirements.

PR #36 made this same call independently (its SPEC-001 diff: *"This is an architecture change: the current single composite-action shell flow is not assumed to provide this boundary"*). We're adopting that direction. **Question for you:** do you agree the job-level-only nature of GitHub Environment protection makes this non-optional, or is there a mechanism we're missing that could gate step-level credential access some other way (e.g., a manual-approval Action, `workflow_dispatch` with an approval bot) that would let SPEC-001 legitimately stay same-job?

---

## 2. PR #36 vs. our branch — not a strict "one wins," a synthesis

| Area | Our branch | PR #36 | Decision |
|---|---|---|---|
| Job split (001) | Forbids (wrong) | Owns (right) | Adopt PR #36's direction |
| Digest/canonicalization detail (001 AC-001) | SHA-256 + `olpc-cjson`, explicit field-omission rules, explicit non-self-reference statement | JCS-based, similarly explicit but independently derived | Keep ours — more implementation-ready, same rigor |
| `SignaturePolicy`/`ProvenancePolicy` enforcement point (002 AC-004/005) | Type exists, no required placement or enforcement — your MEDIUM finding | Names the release report as consumer, adds explicit `mode`/`failure` semantics | Adopt PR #36's shape |
| Requirement-authority gap (all 6 specs) | Every round says "leave `linked_requirement` as-is, repo-wide gap, out of scope" | Adds `.claude/specs/INDEX.md` making accepted specs themselves normative, plus `status`/`owner_track`/`last_verified_revision`/`depends_on` metadata fields | Adopt PR #36's `INDEX.md` and metadata convention across all 6 specs |
| 004 precision (exact CLI flags, exact exception-record schema, deny.toml's real ignore count) | 4 rounds of `canon:exit-gate`-verified precision (caught: invalid `-f` flag placement, wrong ignore count of 11 vs. real 17, undefined exception schema) | Lighter touch, doesn't carry this precision | Keep ours |
| **001 AC-002/AC-005 stale-plan fallback** (your HIGH finding) | Not fixed | **Also not fixed** — PR #36's diff doesn't touch AC-002 or AC-005 | Neither branch had this; being fixed now (see §3) |
| **004 audit-db-checkout consistency** (your HIGH finding) | Not fixed | Touches AC-001's wording but still records the revision via a separate `git rev-parse HEAD` against the advisory-db checkout, not tied to what `cargo-deny` itself scanned | Neither branch fully closes this; being fixed now (see §3) — **flag if you think PR #36's version already closes this, we may be misreading their diff** |

---

## 3. What's being fixed right now (in progress as of this report)

Four `canon:drafter` agents (opus, high effort) are rewriting 001/002/003/004 to:

1. **001**: flip non_goals to own the job split (same-job handoff kept as a valid degenerate case for non-hardened setups); make the AC-002/AC-005 contradiction unambiguous — an explicit durable-plan `apply`/execute invocation must treat a validation mismatch as **terminal** (no mutation, no silent fallback to a freshly recomputed plan); the "reconcile from fresh observations" behavior applies only to a distinct, explicitly-requested reconciliation/status command, never as automatic fallback inside an apply that was handed a specific plan to validate.
2. **002**: adopt PR #36's `ProvenancePolicy` enforcement-point fix; update the scope note since the HEAD==source_commit check is now always load-bearing (not conditional on an uncertain 001 architecture decision).
3. **003**: formalize `depends_on: [001, 002]`; evaluate folding in PR #36's "workflow YAML reference alone is insufficient evidence" framing alongside our existing out-of-band `gh api` verification criterion without duplicating it.
4. **004**: make one advisory-db checkout authoritative for both the revision-recording step and the actual `cargo-deny` run (investigating whether `cargo-deny` can be pointed at a pre-fetched local db path, or whether the revision must instead be read from cargo-deny's own cache dir *after* it runs, sharing its one fetch).

005/006 will get the `status`/`owner_track`/`last_verified_revision`/`depends_on` metadata fields and an adopted `INDEX.md` for consistency once 001-004 land.

---

## 4. Open questions for your independent judgment

- Does our job-split reasoning (§1) hold, or is there a lighter-weight mechanism we're missing?
- For 004's audit-db-checkout consistency: does PR #36's version actually close this, or were we right that it doesn't? (We may be misreading their diff — worth an independent read.)
- Is `.claude/specs/INDEX.md`'s conflict-resolution rule ("first reproduce the behavior against the revision named by the current-description material... do not preserve known contradictions merely to retain a narrative") sufficient, or does it need more teeth given how many rounds of contradiction we've already found by hand?
- Anything in PR #36 we're wrongly deferring to, or wrongly keeping our own version over, that you'd weigh differently?

This is still spec-only — no implementation has started on any of these six tracks.
