# PR #101 test-quality and coverage audit (HEAD 4d05e40d, base 5e452d04)

Verdict: the PR's tests are mostly black-box at the CLI seam, but thin at the boundaries the incident crossed. I mutated 18 high-value lines in a throwaway worktree (since removed). Against the PR's own tests, 16 of 18 survived; the only kills were F (a white-box unit test) and H (one black-box e2e). The proposed tests kill 15 of the 16 survivors (M has no test). Probing also confirmed 3 defects and 3 harness gaps (section 6).

## 1. Real runs

A second agent (parent PID 98675) ran three cargo commands at 11:58:16 despite the sole-cargo rule. My first model run took 8:53 wall, mostly lock wait.

- `cargo test -p callisto-model`: exit 0, `189 passed; 0 failed`, doctest `1 passed`.
- `cargo test -p callisto-graph`, attempt 1 (default git config): hung at `Running tests/apply_tests.rs` until my 590s timeout (`exit=124`). This is the known global-gpgsign hang.
- Same command with `GIT_CONFIG_GLOBAL=<empty file>`: exit 0, 28 result lines, 574 passed, 0 failed, 0 ignored (lib 447). A second attempt hit my 590s timeout during cold linking; the third, with a 1700s timeout, finished in 17:19.
- `cargo test -p callisto-cli --test durable_release_e2e_tests` (default git config, no hang): rtk printed `cargo test: 11 passed (1 suite, 5.18s)`. Raw listing from an identical worktree build: `running 11 tests ... test result: ok. 11 passed; 0 failed`.
- `.github/tests/release-workflow-behavior/run.sh`: just runs that same cargo test with fake providers. It publishes nothing and needs no credentials. 11 passed, 4.84s. It covers only the CLI, not workflow YAML or its shell.
- `test-release-artifact-build-script.sh`: `PASS: shared release artifact build script validates all declared tuples`, exit 0 (the `wrong.tar.gz` stderr line is the expected negative case).
- `verify-release-workflow-contract.sh`: silent, exit 0.
- Also ran `.github/actions/setup-callisto/tests/test_download_extraction_format.sh`: 5 PASS (stubbed).

## 2. Inventory

**Black-box** (compiled binary, real git repo, fake `cargo`/`git push`/`gh` on PATH; `durable_release_e2e_tests.rs`, all added):
- `product_release_rejects_an_unconfigured_profile_before_writing_intent` (`:424`); asserts message with `contains`.
- `product_artifacts_are_uploaded_once_and_recovered_from_provider_observation` (`:742`).
- `explicit_recovery_reconstructs_missing_state_from_remote_evidence` (`:836`).
- `newer_coordinator_executes_and_recovers_an_older_release_source` (`:885`); asserts receipt orchestration and source revisions.
- `merged_release_commit_executes_exactly_once` was changed: it now checks the receipt file exists and, on a second run, per-effect counts. That weakens the old byte-identical log check.

**White-box** (private helpers or types):
- graph `release.rs`: `product_asset_names_are_target_qualified_and_complete` (tautology: copies the code's table), `profile_registry_route_replaces_the_logical_crates_io_destination`, `github_api_response_preserves_a_parseable_not_found_on_nonzero_exit`, `github_release_status_only_proves_absence_for_not_found`.
- graph `release_execution.rs`: `interrupted_operation_converges_only_from_an_exact_provider_observation`, `missing_state_reconstruction_only_adopts_exact_provider_effects`, `quiescent_incomplete_state_is_never_successful`, `every_verified_terminal_success_allows_completion`.
- model `release.rs`, public API but constructor and validator only, none through serde:
  - `release_run_provenance_binds_both_revisions_and_the_exact_intent`.
  - `artifact_release_receipt_provenance_requires_the_verified_manifest_digest`.
  - `release_run_provenance_and_observation_wire_fail_closed`: misnamed, it never touches serde and checks trivial predicates.
  - `receipt_is_bound_to_intent...`: gained one Indeterminate assertion.
- `cli.rs`: `release_artifact_manifest_requires_all_explicit_paths` (clap parse `is_ok`/`is_err`).

**Semi-black-box** (public loader, message-substring asserts): graph `resolve.rs` `product_release_profiles_bind_named_forge_destinations`, `..._reject_a_shared_forge_destination`, `..._reject_a_shared_registry_destination` (same-key case only).

**Static-grep:** `verify-release-workflow-contract.sh` uses `grep -Fqx` on exact YAML lines. Brittle; proves no runtime behavior.

**Stubbed shell:** `test-release-artifact-build-script.sh` (4 valid tuples, 1 invalid) and `test_download_extraction_format.sh`.

**Other:** the `tty.rs` env-dependent test was deleted; the `snapshot.rs` fixture now asserts its commands succeed. Spec drift: `release_lifecycle.rs` (CLI) and `release_observation.rs` (graph) named in the plan do not exist.

## 3. Behavior matrix (happy | corner | error)

- **Intent immutability and profile digest binding:** weak (all fixtures use "production") | NOT (no test that the digest changes with the profile) | NOT (no relabelled-profile intent test; none for `execute --profile` vs intent, `cli:302`).
- **Observation outcomes:**
  - absent and exact: covered by the e2e recovery tests.
  - conflict: weak (only the pre-existing tag dispatch test); forge and artifact conflict arms are uncovered.
  - indeterminate: weak (status helper only); cargo/npm/PyPI arms of `observe_prepared` untested.
  - Cargo and npm observation can only yield Exact or Absent, so SRL-07 "same version, different contents is a conflict" has no test and no implementation.
- **AlreadySatisfied:** covered by effect counts and one white-box test; the receipt outcome value is never asserted.
- **Attempting stranded gives non-zero:** weak (`failed_publish_persists_indeterminate_attempt` runs once, `contains("attempting")`); NOT for a rerun. `recover_interrupted_operations` loop body (`release_execution.rs:163-165`) has zero coverage.
- **Resume on a fresh runner:** weak ("fresh" = delete state and receipt in the same checkout after full success); NOT for partial provider success then recovery.
- **Full SHA / abbreviated / unknown SHA:** full SHA covered; NOT at the CLI for abbreviated or unknown (`CommitSha::parse` unit tests predate the PR).
- **Coordinator vs source provenance:** covered black-box; receipt `kind` never asserted (mutant C).
- **Artifact manifest and attestation, 4 slots:**
  - Covered: happy path (4 slots, 4 uploads); extra unlisted files ignored (`.hidden-metadata`, `nested/`).
  - NOT covered: the model attestation-source rule (mutant R); tampered bytes, tampered manifest, or missing artifact at the CLI.
- **Receipt only after global terminality:** weak (receipt-exists check plus one model case); NOT for fresh-evidence gating (mutant G).
- **Profile and registry-route validation, shared destination:** covered for shared forge and shared registry key. NOT for shared URL under different keys (J), unknown route, missing URL, bad target roster, invalid profile name, or bare `product-package` (6c).
- **Unconfigured profile fails before writing intent:** covered only when `[release]` exists; otherwise it succeeds (6b).
- **Credential-free endpoint digest:** only the pre-existing binding test (not in the diff).
- **Tag, GitHub Release, and registry agreement:** happy only. Conflicting tag or forge release: not covered.
- **Partial failure then recovery:** NOT covered. **Double execution:** covered.
- **Concurrent execute, tampered state, tampered intent:** NOT covered in the PR (concurrency probed OK, 6h).

## 4. Coverage (cargo-llvm-cov 0.9.0; model lib + graph lib + durable e2e only)

PR-added instrumented lines covered:

| File | Diff lines covered |
|---|---|
| model `release.rs` | 89.8% (27 uncovered) |
| graph `release.rs` | 72.6% (137 uncovered) |
| graph `release_execution.rs` | 92.6% |
| graph `release_artifacts.rs` | 90.5% |
| graph `resolve.rs` | 85.4% |
| cli `commands/release.rs` | 62.1% (97 uncovered) |

Uncovered functions and branches:
- graph `release.rs`:
  - `observe_prepared`: Cargo pre-check error, npm, PyPI, unsupported, tag Conflict (`:521`), forge Conflict and Indeterminate.
  - `dispatch_registry` pre-check errors (`:566-569`, `:582-585`).
  - `dispatch_forge_release` Indeterminate (`:873-875`, `:901-907`).
  - `dispatch_artifact_upload`: nearly all failure arms.
  - `observe_artifact_upload`: Conflict and malformed arms.
  - `github_api_response` error arm.
  - `artifact_policy_from_intent` mixed-policy error.
  - `prepared_registry_binding` errors (`:1808-1834`).
- cli `release.rs`: all `artifact_manifest` error branches; `plan` absolute decision path and bad orchestration or repository args; `execute` invalid orchestration and Hermetic source branches.
- model `release.rs`: receipt `validate_for_intent` (`:2195-2205`), receipt wire validation (`:2234-2245`), `UnsupportedSchema` and `NonGit` provenance errors.
- `resolve.rs`: `resolve_product_release` errors (`:155-156`, `:170-172`) and `validate_release_profile_routes` (`:593-611`).

## 5. Discrimination: 18 mutants

"PR tests" = model lib, graph lib, and the durable e2e file.

| Mutant | PR tests | Killed by proposed |
|---|---|---|
| A: delete `require_terminal_success` call (`release_execution.rs:89`) | SURVIVED | p17 (asserts E172) |
| B: delete `recover_interrupted_operations` call (`:72`) | SURVIVED | p01 (asserts E173) |
| C: `recovery && state_was_missing` to `state_was_missing` (`:69`) | SURVIVED | p03 |
| D: drop `profile` from the intent digest (`model release.rs:1443`) | SURVIVED | p04, model test |
| I: remove profile-mismatch check (`cli:302`) | SURVIVED | p05 |
| V: remove forge-repository check (`cli:216`) | SURVIVED | p19 |
| N: remove artifact-repository vs remote check (`graph:1359`) | SURVIVED | p18 |
| J: ignore shared registry URL (`resolve.rs:627`) | SURVIVED | config test |
| K: forge Indeterminate mapped to Absent (`graph:536`) | SURVIVED | p09 (asserts E173) |
| L: tag Conflict mapped to Exact (`graph:521`) | SURVIVED | p14 |
| M: PyPI Indeterminate mapped to Absent (`graph:507`) | SURVIVED | none written |
| O: skip asset digest comparison (`graph:1037`) | SURVIVED | p16 |
| E: skip receipt provenance validation in `from_evidence` | SURVIVED | model test |
| G: synthesize Exact observations in the CLI (`cli:421`) | SURVIVED | p15 |
| R: drop manifest attestation-source check (`model:1246`) | SURVIVED | model test |
| P: remove the cargo pre-publish guard | SURVIVED | p03 |
| F: receipt accepts a non-Exact observation | KILLED (white-box `receipt_is_bound...`) | n/a |
| H (control): orchestration revision replaced by source | KILLED (`newer_coordinator...`) | n/a |

A/B and V/N are redundant safety nets, so a single-mutant survivor is defense in depth. They are killed only by asserting the typed diagnostic (E172, E173, E175).

## 6. Defects and gaps reproduced by probes on HEAD

a. **Malformed `--orchestration-revision` is validated after effects run** (`cli release.rs:382` executes, `:390-391` parses). `execute --orchestration-revision not-a-sha` runs `cargo publish` (count 1), then errors with no receipt.
b. **Unconfigured profile succeeds without `[release]`:** `plan --profile does-not-exist` exits 0 and writes an intent (`cli:243` arm). `docs/07-self-release-lifecycle.md:50` says it fails first.
c. **Bare `product-package = "core-crate"` is accepted** although the error text demands a qualified id (`resolve.rs:154-155`). The plan then has `artifactSlots=0`: the four binary assets are silently dropped (`graph:1537`).
d. **Tag observation is local-ref only** (`observed_tag`, `graph:1048`, `git rev-parse refs/tags/...`).
   - The harness's fake `git push` swallows the push, yet the receipt records the tag as `exact`.
   - Deleting the local tag and recovering re-creates and re-pushes it (`git push` count 1 to 2, exit 0).
   - Workflow `fetch-depth: 0` masks this in production, but no test can see a push that never landed.
e. **A pre-existing lightweight tag yields E164 "malformed output"**, not a typed conflict.
f. **A conflicting annotated tag is found only after `cargo publish` ran** (publish count 1). PLAUSIBLE ordering weakness against SRL-06 "before every effect".
g. Fresh-runner tests reuse the same checkout and local git state.
h. Concurrent execute is safe (second process gets E051 workspace lock, publish count stays 1). Source and coordinator worktrees stay clean after plan and execute (passed).

## 7. Proposed tests, by risk

The proposed tests (a CLI, a model and a graph config file) were integrated into the crates and the proposed-tests folder was removed. They reuse the existing e2e helpers. Run on HEAD: CLI 23 new tests, 19 pass and 4 fail (p06, p08, p20, p21, each a confirmed defect); model 5 of 5 pass; graph config 5 of 6 pass (bare `product-package` fails).

1. **p06** malformed orchestration revision rejected before any effect. Assert non-zero and `cargo publish` count 0. FAILS on HEAD (6a).
2. **p01** stranded Attempting rerun: publish fails, swap in a healthy fake, rerun. Assert non-zero, stderr `E173`, publish count 1, no receipt, state still `attempting`.
3. **p02** partial success then fresh-runner recovery: create the cargo marker, run `--recovery` with no state. Assert exit 0, no `cargo publish`, one `gh release create`, tag present, receipt `kind == recovery`, registry op `alreadySatisfied`.
4. **p03** lost state without `--recovery`: non-zero, no extra publish, no receipt.
5. **p04/p05** tampered intent profile (digest failure, no effects) and `--profile rehearsal` against a production intent.
6. **p14** recovery with a conflicting annotated tag: E173, no receipt.
7. **p15** receipt needs fresh evidence: after success delete the forge marker and receipt, rerun. Assert non-zero, no receipt.
8. **p16** remote asset with the same size but a different digest: E173, no new uploads, no receipt.
9. **p09** forge 401 stub during recovery: E173, no `gh release create`, no receipt.
10. **p12** tampered bytes, missing artifact, tampered manifest digest: each rejected before any effect.
11. **p17** tampered `failed` state gives E172.
12. **p18/p19** artifact repository differing from the git remote (E175) or from the profile forge repository.
13. **p08** unconfigured profile without `[release]` rejected (fails, 6b). **p07** abbreviated and unknown SHA rejected (passes).
14. Model tests: profile bound into digest and not relabellable; receipt rejects provenance for another profile or intent; duplicate, missing, or non-Exact observations (constructor and serde); attestation source must equal coordinator revision.
15. Graph config tests: shared URL under two keys, unknown route, missing URL, bad target roster, invalid names, plus bare `product-package` (6c).

Not written: PyPI and npm observation (mutant M), and a tag-observation test against a real bare remote (6d).
