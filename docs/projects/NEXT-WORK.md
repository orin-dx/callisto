# Next work after 0.10.2

Baseline: main `07154021c` (release 0.10.2 published 2026-10-14). Written as a handoff: a fresh session should read this file, then only the contracts and code each brief names. Delete a workstream when it ships, and this file when all do.

## State of the world

- Published: `callisto-model`, `callisto-manifests`, `callisto-graph`, `callisto` at 0.10.2 (tags `callisto@0.10.2`, `callisto-{model,manifests,graph}@0.10.2`; GitHub release with three binaries). 0.10.1 was never published; its changes shipped in 0.10.2. `callisto-fixtures` is `publish = false`.
- The v1 hardening batch (T01-T05, T10-BINS/AUTH, T11, T02, T03 family, T04 family) is shipped. Open v1 defects are the rows under "Open confirmed defects" in [V1-FINDINGS.md](V1-FINDINGS.md): SYS-OWNER, SYS-DEMOTION, SYS-RETRY, SYS-BARRIER, SYS-NAPI, SYS-MATURIN, SYS-PYFILES, SYS-PYROUTES.
- Local gate: `just ci` runs about 1,900 tests with line coverage a little above 91% against a 90% gate. Hosted CI is green.
- Release process: merging to main refreshes the bot's release PR; its CI runs sit at `action_required` until the owner approves the workflow runs; merging that PR publishes. A failed publish fails before any effect and is recovered by a `workflow_dispatch` of the Release workflow with `release_source_sha` set to the merged release commit.

## Rules for every workstream

- Read `AGENTS.md`, `ARCHITECTURE.md` and the matching `docs/architecture/` topic first. Read `docs/adr/README.md` before touching release flow, Git access, publishing or manifest writes. Specs (`docs/specs/*.json`) change in the same PR as behavior; one changeset per user-visible topic, targeting `callisto-cli`; no changeset for pure refactors or tests.
- One `just ci` at a time on the machine, run as `env -u FORCE_COLOR just ci` (this shell exports `FORCE_COLOR=3`, which fails two release tests). Read the log's own `exit=` line, not the background-task notice. Never run cargo builds in parallel inside one checkout; parallel lanes use separate worktrees and run their `just ci` serially.
- Prove behavior with the real tool wherever a fake could hide the bug. This session's release failed because every test used a fake `gh`. Credentials, auth, registry and forge paths need either a real-tool check or a harness fake that refuses to run exactly as the real tool does.
- Write the failing test first and confirm it fails on the old code (stash the fix, run, restore) before claiming the fix. Test with adversarial inputs, not only the case being fixed.
- For refactors, the existing tests are not proof for branches no test executes: check coverage of the code being moved (`cargo llvm-cov --workspace --lcov`) and add characterization tests first.
- Stacked PRs are native GitHub stacks (`gh stack link <PRs bottom to top>`); each PR must pass `just ci` on its own. Push over SSH (`origin` is `git@github.com:orin-dx/callisto.git`). Never force-push; add a commit or merge instead. No session links in commits or PR bodies. One line per Markdown paragraph.
- Worktree sessions reject complex shell: write scripts to `$CLAUDE_JOB_DIR/tmp` and run them by literal path, and use `/usr/bin/git`.

## Workstreams

| ID | Work | Lane | Depends on | Owner decision |
| --- | --- | --- | --- | --- |
| W0 | Cleanup and docs refresh | A | none | none |
| W1 | Test debt: apply mixed-routing, real-service credential check | A | none | where the real-service check runs |
| W2 | SYS-RETRY: rerun after a failed tag push | B | none | none |
| W3 | SYS-PYFILES then SYS-PYROUTES | C | none | yank and conflict semantics |
| W4 | S4: redacted provider output | C | W3 | allow-list shape; wire change |
| W5 | R3 then SYS-BARRIER | D | none | none |
| W6 | R5 then SYS-NAPI | E | none | none |
| W7 | R1: `walk.rs::build` stages | F | none | none |
| W8 | SYS-MATURIN | G | D01 | version authority model |
| W9 | SYS-OWNER, SYS-DEMOTION, resolver divergence | G | W2, D01 | canonical reference model |
| W10 | Smaller decisions: distinct target declarations, Python dev groups | H | none | both |
| W11 | Verification: T12 integration, T12-DX timing, Python hosted publish | I | W2-W9 as they land | credentials and timing run |

## Parallelism and stacks

Lanes are independent (disjoint files) and can run in separate worktrees at the same time. Inside a lane, PRs are stacked in the order listed.

- Lane A (main, independent small PRs): W0, W1.
- Lane B: W2. It touches `release_decision.rs` and tag selection, so W9 waits for it.
- Lane C: W3 (PYFILES, then PYROUTES), then W4. All in the registry provider; W4 changes the persisted observation wire, so it is its own decision and PR.
- Lane D: R3 (moves only, snapshots first), then SYS-BARRIER, both in `release/derive.rs`.
- Lane E: R5a (graph `init.rs` tests to a sibling file), R5b (templates), then SYS-NAPI, all in graph `commands/init.rs`.
- Lane F: R1, `walk.rs`. W9 should follow it because both touch identity assignment.
- Lanes G, H, I wait on owner decisions.

Do not run two lanes that edit the same file at once. Cross-lane conflicts: W9 after W2 and R1; W4 after W3; SYS-BARRIER after R3; SYS-NAPI after R5.

## Briefs

### W0 Cleanup and docs refresh

- Delete merged remote branches (`git branch -r --merged origin/main`; the stack and fix branches from the 0.10.x work) and local branches and worktrees under `.claude/worktrees/`. Look at each before deleting.
- Refresh [ROAD-TO-V1.md](ROAD-TO-V1.md): its baseline is `8ef9aa5a4`, Stage 1 (restore the release path) and T01/T11 are shipped, and v0.9 is not the priority any more.
- Prune [V1-FINDINGS.md](V1-FINDINGS.md): remove "Implemented locally, awaiting merge" and the v0.9 handoff, keeping open defects and evidence gaps. Verify every row against the code before removing it.
- [V1-IMPLEMENTATION.md](V1-IMPLEMENTATION.md), [V1-VERIFICATION.md](V1-VERIFICATION.md) and [V1-SYSTEM-HARDENING.json](V1-SYSTEM-HARDENING.json): `AGENTS.md` says delete a plan once its work ships. Remove shipped task packets and batch evidence; keep the open ones.
- Done when: docs match the code, `just ci` passes (the docs checks), no changeset.

### W1 Test debt

- Apply mixed-routing characterization. `crates/callisto-graph/src/apply.rs` has `write_steps` and `apply_write_step`; the `MixedManifestVersion`, `MixedWorkspaceVersion`, `MixedManifestRewrite`, `MixedWorkspaceRewrite` and `WorkspaceRoot` steps are never executed by any test (confirmed from a line-coverage report). Build a plan where one manifest receives both `Manifest` and `CargoWorkspacePackage`/`CargoWorkspaceDependency` writes (an inherited-version Cargo workspace root that is also a member) and assert the exact bytes and one persist per physical file. The step order must match the pre-refactor loops: batched manifests, workspace roots, mixed-routing versions, mixed-routing rewrites, platform writes, optional dependencies.
- Real-service credential check. `runner::tests::the_runner_gives_the_real_gh_its_token` is opt-in (`--ignored`) and needs `GH_TOKEN`, an empty `GH_CONFIG_DIR` and `CALLISTO_TEST_ATTESTED_ASSET`. Decide where it runs in CI (the callisto CI workflow, with a downloaded latest-release asset and the job's read-only token) without giving it registry secrets. The release workflow policy script enforces who gets secrets (`.github/tests/verify-release-workflow-policy.sh`); do not weaken it.
- Done when: both are in CI or documented as opt-in, each fails against a deliberately broken variant.

### W2 SYS-RETRY (rerun after a failed tag push)

- Defect, reproduced in the register: `derive_unreleased_decision` in `crates/callisto-graph/src/commands/release_decision.rs` excludes a package because a local tag exists before the remaining operations are observed. Tag created, push fails, a fresh `callisto release` exits 0 with "Nothing to release." and no receipt.
- Contracts: REL-RERUN-02, REL-CMD-01/08 in `docs/specs/release.json`; ADR 2 and 3; `docs/architecture/release.md`. Callisto's own committed-roster CI does not use this path; generated simple CI does.
- Approach: selection must observe every operation of a package, not infer completion from a local tag; "nothing to release" only when every operation is observed satisfied. Keep the committed-decision authority intact.
- Tests (black box, real binary, release harness in `crates/callisto-cli/tests/common/release_harness.rs`): local tag exists, push fails, rerun publishes the rest and issues a receipt; fully released stays "Nothing to release".
- Done when: the reproduction in the register passes, specs updated, changeset ("a rerun after a failed push completes the release").

### W3 SYS-PYFILES then SYS-PYROUTES

- PYFILES: `provider/registry.rs` returns `Exact` for any matching non-yanked Python file and `provider/mod.rs` adopts it as satisfied, so a partial wheel/sdist upload is adopted as complete. The argv module builds both wheel and sdist. Required outputs must be the full selected file set; PyPI yank is per file (any matching yanked file currently reads as Absent even next to non-yanked files).
- PYROUTES: `PreparedRegistryBinding.endpoint` is reused for simple-index observation and Twine upload; servers with separate read and upload routes cannot use one override.
- Contracts: REL-PUB-04/08, REL-RERUN-01/04, `docs/architecture/release.md`, ADR 4; the register's "Integration risks" bullets on completion and yank semantics.
- Tests: fault-injected partial upload (the repo's fake-registry harness, `release_real_registry_e2e_tests.rs` for the real local registry), file-by-file; do not adopt a partial set. Real Python build was never exercised (no network in the audit): run it with real `uv build` if available.
- Decision needed: conflict and yank semantics for a partially present set. Do not guess a platform matrix or checksum policy.

### W4 S4 redacted provider output

- npm observation discards stdout and stderr; an auth failure surfaces as "indeterminate, no details". Add optional `details` to the indeterminate observation, populated from `redact_native_output`.
- Code: `provider/registry.rs` (the `transient(...)` call sites), `crates/callisto-model/src/release_observation.rs` (`ProviderIndeterminateCause`). The model doc states no arbitrary provider output is persisted; keep it true.
- Redaction must be an allow-list, not a deny-list: the receipt is committed, so a miss leaks a credential permanently. Keep a last-resort class (exit code, HTTP status line) so a novel safe failure still gives a signal. Version the allow-list in `docs/specs/release.json`.
- This is a persisted wire change: update `callisto schema --type release-receipt` expectations (`schema_guard_test.rs`), the spec, and add a changeset. Test with adversarial inputs (token in a URL, `.npmrc` echo, header line, JSON body) and assert no fixture token appears in any serialized observation.

### W5 R3 then SYS-BARRIER

- R3: decompose `derive_release_inputs_with` (412 lines, `crates/callisto-graph/src/commands/release/derive.rs`) into selection, remote preparation and operation building, with an `OpIndex` for the four prerequisite maps. Moves only, no reordering of construction: the intent digest covers the operations, so a reorder changes every durable intent.
- First add insta snapshots (insta is a workspace dev-dependency) of the serialized intent and the ordered operation-ID list for representative workspaces, then move code. The snapshots must pass unchanged.
- SYS-BARRIER (after R3, REL-ORD-01): a dual Cargo/npm owner creates the shared tag and GitHub publication after one distribution's publish but before the other's. Give the owner tag a prerequisite on every required publication (`OpIndex::tag_prerequisites`). Behavior change: spec, changeset, and a black-box dual-manifest test.

### W6 R5 then SYS-NAPI

- R5a: move graph `commands/init.rs`'s inline `mod tests` (starts near line 1163 of a 2,459-line file) to `init/tests.rs` (pure move; `callisto-cli`'s init already uses the pattern). Architecture scanners already skip `tests.rs`.
- R5b: move the `WORKFLOW_TEMPLATE` and `MATRIX_WORKFLOW_TEMPLATE` string consts (around lines 796-1070) to `crates/callisto-graph/src/templates/*.tmpl` loaded with `include_str!`, placeholders (`__BRANCH__`, `__COMMIT__`, `__VERSION__`, `__RELEASE_ENV__`) unchanged. Use `.tmpl`, not `.yml`: a placeholder file must not look like a workflow, and `@__COMMIT__` is not a valid `uses:` ref. First pin the exact rendered bytes for both workflow shapes with insta (find the render function from the template consts' callers), then move; also lint the rendered output. The policy and contract scripts consume the generated file.
- SYS-NAPI (after R5): generated native builds and placement use workspace-wide napi selection instead of the verified intent, so an unrelated addon can block a Cargo-only release. CLI-WF-04. Derive the roster from the intent; test with a workspace holding an unrelated napi addon.

### W7 R1 `walk.rs::build`

- `build` in `crates/callisto-graph/src/walk.rs` is about 590 lines. Stages: discovery, platform owners (already its own function), identity assignment, platform attachment, package resolution with rules, zero-match diagnostics, edge construction.
- Constraints: identity assignment is one incremental, order-dependent loop (a path's bare versus prefixed identity depends on names seen earlier in path-sorted order). Own stages by value (`Discovered`, `Identified`, `Resolved`), keep the seven-map bookkeeping private to the identity stage, and make `promotion_rekey` a method on a private struct that owns the index and bookkeeping. `promoted_siblings` is an output of identity assignment.
- Invariants to preserve: E100 fires on any declared (ecosystem, name) owned by a different path, before promotion bookkeeping; promotion demotes the bare name; priority order uses the named function, never enum discriminants; `[[package]]` beats `[[package-set]]`; rule matching never sorts. `walk/tests.rs` must pass unchanged; add a boundary test for the promotion re-key if uncovered (production coverage of `walk.rs` is high, so check).

### W8 SYS-MATURIN (blocked on D01)

- Two fresh co-located Cargo/Maturin repos (matching static versions, Cargo-owned dynamic Python version) fail with E007 from init and status. `Package::version_grammar`; WS-ID-09; model in `docs/architecture/polyglot-v1.md`, ADR 11 and the proposed ADR 12. Needs the owner-contract (D01) decision on version authority before code.

### W9 SYS-OWNER, SYS-DEMOTION, resolver divergence (blocked on D01, after W2 and W7)

- `sole_owner_at` uses current first-manifest path existence instead of historical name and roster ownership, losing a bare tag after a sibling rename; `tags.rs` loses a newer qualified tag after demoting a sibling and picks an older bare release.
- Resolver divergence: `PackageId::resolve_unique` (`crates/callisto-model/src/identity.rs`, used by `commands/status.rs`) versus `IdentityIndex::resolve_id` (version aggregation). Add a paired fixture first (a dual-manifest package whose Cargo and npm names differ, a changeset naming the npm name; status and version must agree). Which side migrates depends on whether `docs/specs/workspace.json` defines display-level matching; `resolve_unique` is public API of a published crate, so retire it with a deprecation notice, not a silent removal. Fold into the canonical-reference work; do not build two resolvers.

### W10 Smaller decisions

- Distinct target declarations: the graph reports one edge per (kind, name); a declaration that reuses a key with a different constraint is left untouched and untracked. Fully tracking it needs a discriminator on `RewriteKey` (`crates/callisto-graph/src/cascade.rs`), a public-API change guarded by `check-api`.
- Python `[dependency-groups]` and Poetry groups are not tracked (dev-only). Decide whether they should cascade. Extras (`[project.optional-dependencies]`) were fixed in 0.10.2.

### W11 Verification (T12, T12-DX)

- Integrate representative real-path cases (Cargo workspace, npm/pnpm, Python, Cargo+napi, Cargo+Maturin), and time a fresh first published release against the under-ten-minute goal. Python artifact build and hosted Python publication have never run. Needs credentials and an owner-approved run.

## Owner decisions still open

- D01: owner-contract checkpoint (ownership, compatibility, version authority, registry-output proof). Gates W8 and W9.
- Where the real-service credential check runs (W1).
- Yank and conflict semantics for partial Python uploads (W3); the redaction allow-list (W4).
- `RewriteKey` change and Python dev-group tracking (W10).
- Inferred-only or manual version releases in managed CI (policy section of the findings register).

## Lessons from the 0.10.x work

- Compiles and fake-tool tests are not proof. The 0.10.1 release failed because the runner stopped passing `GH_TOKEN` to `gh`; only the real `gh` showed it.
- Splitting one dirty tree into stacked PRs produced layers that compiled but failed tests and docs checks. Verify each layer with the full tests, or collapse layers that only work together.
- Coarse keys (kind and name) for dependency declarations caused a wrong first fix for Cargo target tables; adversarial review found it. Run an independent review on risky changes before merging.
- `just ci` can report a failing step after its tests pass (coverage runs plain `cargo test`); one macOS-flaky socket test was fixed that way.
