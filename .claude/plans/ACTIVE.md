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

### Track: Manifest trait mutate/persist split (SPEC-MANIFEST-PERSIST-001, "Spec A") — DONE

Spec: `.claude/specs/SPEC-MANIFEST-PERSIST-001.json` (21 acceptance criteria, AC-001 through AC-020 + AC-011b)
Plan: `.claude/plans/manifest-persist-001-plan.json` (25 tasks: T01a-T21c)
Origin: one of the 2 remaining HIGH-severity performance findings from the post-Track-G audit
(apply_version_plan's redundant per-entry open+parse+persist cycles). Scoped down from an original
combined spec that also covered write-batching/grouping after a formal escalation (data-loss ordering
bug in the batching design); split into this safe, zero-grouping mechanical refactor (Spec A, shipped)
and a deferred future track for the actual batching/grouping logic (Spec B, not started).
Implementation: `Manifest::persist` promoted to a required trait method; `write_version`/
`update_dependency_spec`/`update_optional_dependencies` across `crates/callisto-manifests/src/{cargo,npm,python}.rs`
now do pure in-memory mutation only; `apply_version_plan` (`crates/callisto-graph/src/apply.rs`) calls
`persist()` explicitly per loop; `ManifestError::InvariantViolation` (E027) guards CargoToml's
self-delegation edge case; `cascade.rs` invariant (inherited edges never target `DepWriteTarget::Manifest`)
locked in with a regression test.
Exit-gate: PASS (high confidence). All 21 AC verified fresh from disk/code. `just ci` full 7-phase suite
green (fmt-check, lint, test, audit, doc-check, wasm-check, coverage) plus `cargo check -p callisto-manifests
--all-features`. Mutation-tested at the gate (skipped during implementation — process note for next time):
both new `persist()` call sites confirmed load-bearing via direct-deletion testing; all mutation survivors
traced to pre-existing, untouched code. 3 non-blocking gaps recorded (no Python-ecosystem byte-identity
test alongside the Cargo/npm ones; mutation gate ran late; pre-existing survivors in adjacent code).
Commits: 7883c3b (trait promotion) through d7ec596 (T20), fix 235ab55 (clippy), fa73260 (spec/plan committed).

### Track: apply_version_plan write-batching (SPEC-APPLY-BATCH-002, "Spec B") — DONE

Spec: `.claude/specs/SPEC-APPLY-BATCH-002.json` (18 acceptance criteria, AC-001 through AC-018)
Plan: `.claude/plans/apply-batch-002-plan.json` (12 tasks: T01-T08, T07c, T07b, T10, T11, T13)
Origin: the deferred second half of the post-Track-G audit's redundant-open/persist performance
finding — the actual batching/grouping win Spec A was groundwork for.
Design decision: after re-deriving the exact data-loss ordering hazard that killed the original
combined spec (a workspace root Cargo.toml that receives writes through both the `Manifest` trait
and the separate `WorkspaceCargoResolver` type), user chose the lowest-risk of three options —
exclude any physical file receiving writes through both mechanisms from batching entirely, rather
than reordering writes or extending Spec A's mutate/persist split to `WorkspaceCargoResolver` too.
Implementation: `classify_manifest_writes`/`ManifestWriteGroup`/`ManifestWriteClassification`
(new, `crates/callisto-graph/src/apply.rs`) partition a plan's Manifest-trait-routed writes into
per-path batchable groups and a mixed-routing exclusion set via a full pre-scan; `apply_version_plan`
opens each batchable path's manifest exactly once, applies its bump (same skip/write/error
precondition as before) then its rewrites in order, and persists exactly once — processing groups
strictly sequentially (one path fully persisted-or-errored before the next handle opens) to prevent
the stale-handle-clobber hazard. `persist_call_count`/`reset_persist_call_count` added to
`callisto-manifests` mirroring the existing `open_call_count` precedent.
Both the spec (5 drafting/audit rounds, canon-exit-gate passed on retry 2) and the plan (7
vector-challenger rounds) went through unusually heavy adversarial review before implementation —
real bugs caught each round, including a silent-data-loss ambiguity in the persist-trigger condition,
a self-contradictory test fixture, an unconstructible acceptance criterion, a dishonest red-phase
waiver, and flaky process-global-counter tests.
Exit-gate: PASS (high confidence). All 18 AC verified fresh from disk/code. `just ci` full 7-phase
suite green (766 tests, up from 751) plus `--all-features` prechecks on both touched crates.
Mutation-tested at the gate (cargo-mutants run directly by the gate, same as Spec A): 12/13 mutants
caught; the one survivor (a hardcoded `persist_call_count()`) closed with a follow-up precision test.
Core safety property (strict per-group sequential processing, preventing the original spec's
stale-handle-clobber bug) confirmed both by code trace and by killed mutants on the exclusion guards.
Commits: d2b1014 (persist_call_count) through c880280 (T13 evidence correction), c661b38 (mutation
gap closed), a739773/7346f48 (spec/plan committed).

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

### Track 3a/3b1: Workspace-Membership Filtering + Identity-Promotion Core — DONE

Spec: `.claude/specs/SPEC-TRACK3A-WORKSPACE-MEMBERSHIP.json` (34 AC), `.claude/specs/SPEC-TRACK3B1-IDENTITY-PROMOTION-CORE.json` (27 AC)
Plan: `.claude/plans/PLAN-TRACK3B1-IDENTITY-PROMOTION-CORE.json` (22 tasks, T01a–T17)
Implementation: `crates/callisto-graph/src/locate/{membership,ignore_walk}.rs` (Cargo/npm/pnpm workspace membership filtering, 45 tasks), `crates/callisto-graph/src/{identity,walk}.rs` (ecosystem-prefixed identity resolution, disjoint cross-ecosystem promotion, `resolve_native_with_fallback` silent-fallback removal).
This is the master remediation plan's "Track 3b–e" (identity/ecosystem resolution hardening) — all four originally-confirmed live bugs fixed: `IgnoreWalkLocator::projects()` now filters by Cargo/npm/pnpm membership globs; `IdentityIndex.prefixed` populated; `package_manifest_decls` disambiguates same-named cross-ecosystem packages via promotion instead of erroring; `resolve_native_with_fallback`'s silent cross-ecosystem match removed.
Exit-gate: PASS. `just ci` green throughout.
Note: Track 3a (name-vs-path `[[package]]`/`[[package-set]] match` semantics — a *different* item from the same-numbered plan above) is NOT part of this entry — see its own entry below.

### Track 3a: `match` Semantics Decision + Doc Correction — DONE

Decision resolved: docs/00-design.md and docs/01-spec.md deliberately specified path-based `match` patterns (`match = "crates/*"`; the spec's §14 algorithm defined the match target as a workspace-relative path) — a genuine, deliberate doc/code mismatch, not a typo. Confirmed via investigation: no real config anywhere in the repo uses path-style patterns, and the shipped implementation (`PackagePattern`, `crates/callisto-graph/src/config/pattern.rs`) already has a deliberate, tested name+ecosystem-prefix glob mechanism (`"cargo:pkg-*"` vs `"npm:pkg-*"`, with its own dedicated `BareRuleMatchesMultipleEcosystems` ambiguity error). User confirmed this is the intended direction. Resolved as a doc-only fix (no pipeline needed, same category as Track 6-doc/8-doc): corrected the worked example and formal algorithm in both docs to describe name+ecosystem matching, plus fixed an unrelated small inaccuracy found while sweeping (`BareRuleMatchesMultipleEcosystems`'s own doc comment used `cargo/name`, a slash, instead of the real `cargo:name` colon syntax).
Commit: `b190626`.

### Track 6-doc / 8-doc: Stale Spec Claims (I/O boundary, Python ecosystem status) — DONE

Doc-only corrections, no code change, no pipeline needed: `docs/01-spec.md` §M.1.2 corrected ("no I/O" claim predates `atomic_write`'s move into `callisto-model`; real invariant is license permissiveness); `docs/00-design.md` and `docs/01-spec.md`'s Python-ecosystem status corrected (shipped, default-on, ahead of the "demand-gated" claim both docs made).
Commits: `7122d37`, `2e441fa`.

### Track 1: Fixed-Group Cascade Correctness — DONE

Spec: `.claude/specs/SPEC-TRACK1-FIXED-GROUP-CASCADE-CORRECTNESS.json` (16 AC — AC-001–013 plus AC-007b/AC-010b/AC-014)
Plan: `.claude/plans/PLAN-TRACK1-FIXED-GROUP-CASCADE-CORRECTNESS.json` (12 tasks, 4 batches)
Fixes three confirmed-live bugs: (1) fixed-group siblings bumped independently from their own base version instead of converging on a shared group-aligned target — fixed via a new Fixed-group convergence block in `solve_cascade` (`cascade.rs`), structurally mirroring the pre-existing Linked-group block; (2) `pre_mutation_checks` (divergence/napi-drift detection, already implemented and unit-tested) was never called from any command — now wired into `plan_version`'s real orchestration, with diagnostics merged and divergence errors aborting; (3) `GraphError::ConflictingGroupMembership` was declared but never constructed — now detected in `GroupTable::resolve` via a single map spanning both Fixed and Linked groups.
Spec passed canon-exit-gate on the 4th attempt — round 1 caught a misdiagnosed fix location (the original spec targeted `raise()`'s sibling loop, which the gate proved unreachable for the seed-path scenario; corrected to the new `solve_cascade` block), round 2 found an over-scoped dedup requirement that traced out as not load-bearing and was removed, rounds 3–4 found a borrow-checker error, an incomplete test-file enumeration, and a nonexistent `VersionGrammar` variant.
Exit-gate: PASS (correction below supersedes an earlier premature reading of the same run). Full workspace suite green: 975 tests across 54 suites, clippy/fmt/doc/wasm all clean. Mutation testing (cargo-mutants, scoped to the 4 touched files, 113 mutants) ran to full completion: 42 caught, 32 timeout, 10 unviable, 28 survivors — **zero survivors in Track 1's own new/touched code**. All 28 survivors are in pre-existing, untouched decision logic (`cascade_action`, `caret_covers`, `coverage()` in `cascade.rs`, plus pre-existing `solve_cascade` regions) — a genuine, long-standing coverage gap predating this track, not introduced by it. Verified non-vacuous: 8 mutants were generated inside the new Fixed-group convergence block (lines 384-434) and all 8 were caught — notably, the exact structural analogues of two of those mutants in the pre-existing Linked-group block (`cascade.rs:333/336`) *did* survive, while the new mirrored guards (`404/407`) did not, meaning the new tests are strictly stronger than the code they were modeled on. (An earlier note in this entry, based on a mid-run snapshot before cargo-mutants finished, mischaracterized this as one in-scope survivor — corrected here.) 25 precision tests targeting the pre-existing survivors were specified by the mutator; deferred as backlog, out of this track's scope.
Commits: `bb9d3aa` (CascadeInput.tags) through `a7e5526` (T12), spec/plan committed at `2347446`/`6cc77ec`.

**Known follow-ups from this track's exit-gate** (non-blocking, not yet scheduled):
- `GroupTable::from_groups` (`config/groups.rs:259-289`) is a third, public, currently-test-only path for constructing a `GroupTable` with no conflict check — could construct a table violating the invariant `resolve()` now enforces. Not a spec violation (spec explicitly scoped the fix to `resolve()`), but worth closing if `from_groups` ever gets a non-test caller.
- `fixed_group_target`'s SemVer-only hardcoding (vs. `bump_target`'s grammar dispatch) is now reachable from `plan_snapshot` for an all-PEP440 fixed group, since `plan_snapshot` deliberately doesn't call `pre_mutation_checks` (AC-006) to catch it first. Accepted by the spec's own non_goals; flagged for whoever eventually fixes the PEP440 gap.
- `CascadeInput` gained a required field and is re-exported from a publishable AGPL crate — a semver-breaking public API change, expected and accepted by the spec, but worth noting for this crate's own next release.
- Two minor test-robustness nits: the AC-005 test asserts a version prefix rather than exact equality; the AC-014 test asserts only `is_err()` without pinning the specific error variant.

### Track 2: Platform-Manifest / optionalDependencies Writes — DONE

Spec: `.claude/specs/SPEC-TRACK2-PLATFORM-MANIFEST-WRITES.json` (16 AC — AC-001–013 plus AC-002b/AC-007b/AC-012b)
Plan: `.claude/plans/PLAN-TRACK2-PLATFORM-MANIFEST-WRITES.json` (8 tasks)
Confirmed via adversarial audit that a prior ungated commit (`2d53001`) had wired the *consumption* side (`apply_version_plan` writing `platform_writes`/`optional_dep_updates` to disk) but the *planning logic* deciding what goes into those fields was never built, and had two dormant bugs (lockfile-staging blind spot; missing precondition check risking silent overwrite). This track built the missing planning logic in `plan_version` (reads each Fixed group's `GroupMember::PlatformManifest` siblings, opens the manifest via the same `OpenContext`/`ManifestRole::Canonical` pattern `apply_version_plan` already uses, emits `PlatformWrite`/`OptionalDepUpdate` entries tracking the owner's real bump) and fixed both consumption-side bugs (`active_ecosystems` now also derives from these two fields; `platform_writes`' dispatch loop gained the same current==from/current==target/else precondition the sibling bump loops already had, via a new `PlatformWrite.from` field).
Spec passed canon-exit-gate on the 3rd attempt (caught a false npm-lockfile-refresh claim, an underspecified `OpenContext` derivation that would break workspace-inherited-version Cargo/maturin crates, two criteria asserting on non-observable state). Plan passed vector-challenger with 3 fixes (a missing `bump_by_pkg` construction step, an unspecified owner-manifest resolution mechanism for dual-published packages, incomplete AC-008 error-case coverage).
**Mid-implementation spec correction**: TASK-3's implementer found and correctly reported that AC-002b (a maturin/Cargo platform manifest resolving its version end-to-end through real discovery) was genuinely unsatisfiable — `walk.rs:199`'s platform-manifest discovery is gated to `Ecosystem::Npm` only, with no Cargo branch, confirmed independently. Routed through `canon/correct`: AC-002b narrowed to verify the `OpenContext` resolution logic directly via a fixture-constructed `GroupMember::PlatformManifest` (bypassing walk.rs discovery), with a new non_goals entry deferring the Cargo-discovery extension as separate future work. Correction passed exit-gate on its 2nd round (a vacuous assertion, an unfalsifiable role-forwarding clause, and a stale `purpose` claim). Plan re-amended via vector-planner's amend mode for just the affected task.
A second, smaller instance of the same underlying limitation surfaced during TASK-3/TASK-6: `walk.rs`'s platform-manifest discovery is strictly directory-scoped (Case D: owner + platform manifest must be co-located), so two platform siblings under one true owner can never both be disk-discovered — only the plan's narrative claimed otherwise, not the spec text itself, so no further spec correction was needed; tests use one real Case D sibling plus one fixture-injected sibling, the same technique AC-002b's correction established.
Exit-gate (implementation): `just ci` full 7-phase suite green. `cargo test -p callisto-graph` clean throughout (376 tests at completion).
Commits: `5eb5937` (T1) through `ef1c633` (T6), spec/plan/correction commits interleaved (`2439832`, `262cc43`, `1d360d5`, `f76088e`).

**Known follow-up**: `walk.rs`'s platform-manifest discovery being npm-only (no Cargo/maturin support, and directory-scoped even for npm) is a real, confirmed limitation surfaced twice by this track — worth its own future track if maturin/multi-platform-per-owner support is ever needed for real.

### Track 9: Cascade Correctness — Peer-Escalation & Cross-Ecosystem Rewrite Keys — DONE

Two independent, single-function bugs in `cascade.rs`, sequenced adjacent to Track 1 (shared file) rather than concurrently.

**Bug 1 — peer-escalation ignored true cascading severity.** `cascade_action`'s peer-dependency escalation branch matched on `coverage` alone, ignoring `source: Severity` — every out-of-range peer edge escalated to Major even when the real upstream change was only Patch, with `peer_escalation = true` the documented default. Fixed via a lighter TDD loop (no spec/plan): gated the escalation branch on `matches!(source, Severity::Minor | Severity::Major)`. Test: `cascade_action_peer_escalation_only_fires_for_non_patch_source` (Patch/Minor/Major/None cases). Commit: `a7007d9`.

**Bug 2 — cross-ecosystem rewrite keys built from the wrong identity.** Dependency-spec rewrite keys in `solve_cascade` were built from `PackageId::name()` instead of the dependent's ecosystem-native identity, so a cross-ecosystem rewrite onto a dual-identity (napi-style) package targeted the wrong manifest key — confirmed live, crashed `callisto version` outright on an ordinary Cargo+npm co-located package with `ManifestError::DependencyNotFound`. Initially mis-tiered in the master plan as a narrow single-function fix; direct investigation found it genuinely required `CascadeInput`/`Workspace` field-threading comparable to Track 1's `tags` field (Layer 1/Layer 2 isolation blocks a naive fix using `ws.graph.identity()` directly, since `identity()` is an inherent method not on the `DependencyResolver` trait bound — both functions needing it are generic over `D`). Escalated to full canon pipeline per user decision.

Spec: `.claude/specs/SPEC-TRACK9-CASCADE-REWRITE-KEY-CORRECTNESS.json` (7 AC — AC-001–007). Passed canon-exit-gate on the 3rd attempt — round 1 caught the `ws.graph.identity()` compile-error design flaw described above (reverted to a genuine new owned `Workspace.identity: IdentityIndex` field); round 2 found two more gaps from that same fix (18 unaccounted-for `Workspace` struct-literal test sites across 6 files; a now-contradictory AC-004 exception clause).
Plan: `.claude/plans/PLAN-TRACK9-CASCADE-REWRITE-KEY-CORRECTNESS.json` (6 tasks). Passed vector-challenger on the 2nd round — round 1 found a drifted file:line citation (T2's 8th `CascadeInput` test literal) and a process-contract conflict (T4's revert-probe wording would have falsely tripped `lambda:implementer`'s stop-on-unexpected-green rule); both fixed directly in the plan JSON before implementation.
Implementation: `identity: &'a IdentityIndex` threaded through `CascadeInput`, owned `identity: IdentityIndex` added to `Workspace` (populated in `Workspace::load` via `graph.identity().clone()`), `solve_cascade`'s `RewriteKey.name` now resolves via `input.identity.native_name(&edge.to, eco)` with a `PackageId::name()` fallback when no cross-ecosystem registration exists.
Exit-gate (implementation): full `callisto-graph` suite 471 passed / 0 failed; `cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings` clean across the whole workspace.
Commits: `f265ff0d` (T1, Workspace.identity + 18 test sites) through `ca2aa550` (T6, end-to-end AC-006 proof); T5 (AC-004, full-suite regression confirmation) required no code changes, verified directly with no separate commit.

### Track 4: Release-Pipeline / CI Action Contract Correctness — DONE

Four confirmed-live bugs in the release pipeline's CLI-to-Action contract: (1) the shipped Action script's `status --check` exit-code branching treated exit 0 as "has changesets" (dead code — the CLI's `--check` contract was already correct: 1=pending, 2=none-pending, never 0) while the real pending case (exit 1) fell into an error-abort branch, making release-PR creation unreachable on every real run; (2) `main.rs`'s doc comment describing this contract was itself stale; (3) `callisto tag` reported a fabricated `sha` (the requested target, not the tag's real existing target) when a tag already existed at a different commit, in both apply and `--dry-run` modes; (4) `plan_publish` never populated `ReleaseEntry.changelog_section`, hardcoding `None` despite the already-implemented `extract_section` having zero call sites.

Spec: `.claude/specs/SPEC-TRACK4-RELEASE-PIPELINE-CONTRACT-CORRECTNESS.json` (16 AC). Passed canon-exit-gate on the 5th attempt after unusually heavy adversarial review — round 1 (self-audit) caught missing dry-run/resolve_commit-failure handling and a falsified empty-changelog-section edge case; round 2 caught the dry-run path bypassing the whole tag-sha fix entirely plus an exit-code-0 hole in the Action script; round 3 caught an invented "Info" severity tier (`DiagnosticSeverity` only has Warning/Error), a serde `skip_serializing_if`/null mismatch, and an unconstructible test fixture; round 4 found the changelog-path derivation ignored the already-resolved, user-overridable `Package.changelog` field that `callisto version`'s write side already uses. User consulted at the round-3/4 boundary per canon-exit-gate's own 3-retry escalation threshold; chose one final targeted fix over escalation, since both remaining blockers traced to one root cause.
Plan: `.claude/plans/PLAN-TRACK4-RELEASE-PIPELINE-CONTRACT-CORRECTNESS.json` (10 tasks). Passed vector-challenger on the 1st round after live-code re-verification of every cited symbol.
Exit-gate (implementation): `callisto-graph` 484 passed, `callisto-model` 87 passed, `callisto-cli` 109 passed, all 0 failed; fmt/clippy clean on every touched crate at every task. Two sibling-fixture regressions surfaced by full-suite verification during T3 (stub `CommandRunner`s in `tags_tests.rs` simulating an already-existing tag had no response for the new `resolve_commit` call) — caught and fixed in the same commit, not deferred.
Commits: `2e6f25c5` (T1) through `926c2af4` (T10, final task).

### Track 6-mechanical: JSON Report Field-Shape Alignment — DONE

Lighter TDD loop tier (no formal spec/plan — narrow, well-scoped correctness fixes, one report type per commit, matching this project's established convention for this rigor tier). Covers every report-type field-shape mismatch confirmed against `docs/01-spec.md` by an earlier research pass, resolved after this session's own critical reassessment removed the earlier "bump schemaVersion" assumption entirely: this project has never completed a real registry publish, so there is no external consumer to protect from a wire-format change — every fix here is a pure correctness fix, no version bump.
- **`SnapshotReport` / `InitReport`** — investigated case-by-case per explicit user request rather than assumed; both turned out to be doc-only fixes (shipped code was correct and ahead of stale docs, same category as Track 3a/6-doc/8-doc). Commit: `4ab25d9a`.
- **`StatusReport` / `StatusPackageRecord`** — added missing `hasChangesets` (mandatory, Action's mode dispatch reads it), `lastReleasedVersion`, `releaseTrigger`; removed a `skip_serializing_if` that contradicted the doc's "always present" requirement on `changedSinceLastTag`. All three populated from real data already computed in `callisto-graph`'s status command, not inert placeholders. Commit: `c847e0da`.
- **`ValidateReport`** — `valid` renamed to `ok` (doc-documented key); `diagnostics` now always serialized per the doc's "validate's entire payload *is* its diagnostics" framing. Commit: `c277c561`.
- **`ComposePrBodyReport`** — `pr_body` renamed to `body`. The doc's `metadata: PrBodyMetadata` object (labels/managedLabels/overflow) was investigated and deliberately NOT added: 2 of 3 subfields have zero backing computation anywhere in the codebase (no label round-tripping, no overflow/notes-branch detection) — adding it would mean fabricating placeholder data. Documented as an explicit, scoped deferral rather than silently dropped. Commit: `d6714eec`.
- **`TagReport` / `CreatedTag`** — `created_tags` renamed to `tags`; added `already_existed: bool`. All four `CreatedTag` construction sites in `create_tags_with_options` (per-release tag + floating-major alias, both apply and `--dry-run` modes) already computed this value locally for Track 4's sha-resolution fix — this fix just threads the existing value into the struct. Track 4's own AC-13 schema-drift guard test updated to reflect this intentional shape change. Commit: `85cfd7ac`.

Every fix verified across all crates that construct or consume the touched report type (`callisto-model`, `callisto-graph`, `callisto-cli`, and `callisto-moon` where applicable), fmt/clippy clean throughout.

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
