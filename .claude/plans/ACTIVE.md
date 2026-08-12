# Active Work — Callisto

## Completed

### Track B: Idempotent `apply_version_plan` — DONE

Spec: `.claude/specs/track-b-idempotent-apply.json`
Implementation: `crates/callisto-graph/src/apply.rs`
Exit-gate: PASS. All 6 apply tests green, `just ci` exits 0.

### Track E: PackageId Specificity + Cross-Ecosystem Diagnostic — DONE

Spec: `.claude/specs/track-e-specificity.json`
Implementation: `crates/callisto-graph/src/walk.rs`, `crates/callisto-model/src/diagnostic.rs`, `crates/callisto-graph/src/aggregate.rs`
Exit-gate: PASS (2nd attempt). 620 tests, clippy clean, AC-1 through AC-11 verified.
Commits: T1 ce20787, T2 6cb3130, T3a f408845, T3b+T4 c0fdc96, post-gate fixes f8f168c

---

### Track F: resolve_package_config Extraction — DONE

Spec: `.claude/specs/track-f-resolution-extraction.json`
Implementation: `crates/callisto-graph/src/config/resolve.rs`, `src/walk.rs`, `src/aggregate.rs`
Exit-gate: PASS (high confidence, mutation-tested). 627 tests, clippy clean, AC-F1 through AC-F9 verified.
Commits: T1a+T1b+T1c c53a7e7, T3a+T3b dccf070

### Track G: callisto matrix (napi + maturin auto-discovery) — DONE

Spec: `.claude/specs/track-g-matrix-napi-maturin.json` (7 canon rounds, 4 vector-challenger rounds)
Plan: `.claude/plans/track-g-matrix-napi-maturin.plan.json` (24 tasks)
Exit-gate: PASS. 24 tasks implemented, all reviewed, 3 gaps found and fixed post-exit-gate
(stale docs, missing `callisto schema` arm, missing assertion). `just ci` green.
Commits: eb5d6b4 (spec+plan) through 5164971 (action.yml wiring), then d18dc92 (exit-gate fixes)

### Post-Track-G: full workspace audit + 8-bug remediation — DONE (see SESSION_HANDOFF.md)

Full adversarial audit (8-dimension, 24-agent workflow) found 3 critical + 7 high confirmed bugs,
concentrated almost entirely in pre-existing code committed early this session (`e0cc091`, `60e0bef`)
that never went through canon→vector→lambda — NOT in Track F/G's own freshly-built code. All 8
confirmed critical/high findings fixed via strict TDD, commits `bca04b1` through `def6862`.
Report: https://claude.ai/code/artifact/80f347f5-afea-488c-886b-518bea99a458
**See `.claude/plans/SESSION_HANDOFF.md` for full detail and the recommended next step** (4 named
test-coverage gaps + 41 unverified medium/low findings as backlog).

**Process lesson from this incident** (saved to memory, read it):
code found already-written in a dirty working tree gets the SAME rigor as freshly-written code —
"compiles + existing tests pass" is not verification. See
`feedback_found_code_needs_same_rigor.md` in the memory index.

---

## Pipeline Protocol (follow for every track, in order)

1. `canon:canon-drafter` — write spec@1 to `.claude/specs/`
2. `canon:canon-auditor` — must pass before continuing
3. `canon:canon-exit-gate` — must pass before continuing
4. `vector:vector-planner` — produce plan@1 with exact red tests
5. `vector:vector-challenger` — must pass before continuing
6. `lambda:lambda-recon` — confirm baseline tests pass
7. `lambda:lambda-implementer` — one task at a time, red phase MANDATORY
8. `lambda:lambda-reviewer` — review each commit before next task
9. `lambda:lambda-exit-gate` — adversarial final check
10. `just ci` — must exit 0

Constraints:
- NEVER commit without explicit user instruction
- NEVER invoke `callisto publish` against any real registry
- Confirm test FAILS before writing implementation
- Do not interleave Track B and Track E implementation tasks
