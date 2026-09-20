# Closure matrix: PR 101 release-lifecycle audit findings

Source: an independent adversarial review of the branch, run against the code (not commit messages), with the workflow policy scripts, actionlint and zizmor executed and live GETs to the registries, GitHub and a git remote. The tables below are that review's Task A, kept whole. Two facts changed after it was written:

1. The review's F1, F2, F3, F4 and its top-10 items were fixed in later commits, so seven rows below moved from OPEN or PARTIAL to FIXED (marked inline). The fixes: the recovery reconstruct sweep, the lag-tolerant receipt observation, stderr redaction, loopback-only http registries, package-name validation, bare `--state` path handling, route key validation, execute workspace root resolution, the artifact component dot check, and policy pins for the signed-commit requirement and the dispatch regex (7063b6f7).
2. The review reported six of ten hand-written workflow mutants surviving both scripts. 7063b6f7 (test(ci): pin the signed-commit requirement and the release job guards) added the missing assertions and self-test mutants (verified requirement dropped, dispatch sha regex weakened, resolved-sha equality dropped, execute loses plan success, plan runs always, execute checks out moving ref, handoff step disabled/renamed/continue-on-error, verified query dropped); the policy self-test carries a mutant for each.

Rows still OPEN or PARTIAL need the owner or are listed as deferred in README.md.

### OPEN or PARTIAL at review time (fixed rows marked)

| ID | Status | Evidence |
|---|---|---|
| wf P2-8 transitive mutable action | **OPEN** | `.github/actions/setup-callisto/action.yml:22` pins `orin-dx/actions/setup-rust@2521888…`; I fetched that blob: line 53 is `- uses: Swatinem/rust-cache@v2` (floating). It runs in `build-artifact` (id-token+attestations: write) and `execute` (contents: write, holds `CARGO_REGISTRY_TOKEN`). `verify-action-pins.sh` cannot see it. |
| wf P2-11 managed PR has no checks | **OPEN** | live: `gh pr view 99 --json statusCheckRollup` → `checks: 0`, `mergeable: MERGEABLE`. The 12 required checks never start on the bot PR, so the sole authorized publication act needs a human "Approve and run" or the owner bypass. |
| core P2-1 recovery not resumable | **FIXED** | observe-before-`Attempting` is fixed (`release_execution.rs:72-77`), but `reconstruct_missing_state` runs only `if kind==Recovery && state_was_missing` (`:56`), and `:52` persists state *before* reconstruct. See F1. Review-time evidence above; Fixed after the review in bd630b06 (fix(release): harden recovery, receipts, redaction, registries and package names): the reconstruct sweep now covers every still-Pending operation on any Recovery run. Test: a_recovery_run_interrupted_mid_reconstruction_resumes_from_the_same_state. |
| core P3 unredacted stderr in errors | **FIXED** | `crates/callisto-graph/src/error.rs:622` `NonZeroExit{stderr}` Displays raw child stderr; `CliCommandRunner::run_with_timeout` redacts the *live echo* only. Review-time evidence above; Fixed after the review in bd630b06 (fix(release): harden recovery, receipts, redaction, registries and package names): known secrets and URL userinfo are redacted at the NonZeroExit Display chokepoint. Tests: non_zero_exit_display_redacts_release_path_credentials, non_zero_exit_display_redacts_url_userinfo. |
| cfg P3-10 `http://` registry allowed | **FIXED** | `registry_endpoint.rs:69` accepts `http`; `npm --registry`/`twine --repository-url` would then carry credentials in cleartext. Review-time evidence above; Fixed after the review in bd630b06 (fix(release): harden recovery, receipts, redaction, registries and package names): plain http is accepted only for loopback hosts. Tests: plain_http_is_accepted_only_for_loopback_hosts, plain_http_is_rejected_for_every_other_host. |
| cfg P3-10 logical route keys unvalidated | **FIXED** | `config/resolve.rs` `resolve_release_profile` builds `RegistryKey(logical)` raw although `validated_registry_key` exists (`model/release.rs:1341`). Review-time evidence above; Fixed after the review in bd630b06 (fix(release): harden recovery, receipts, redaction, registries and package names). Test: product_release_routes_reject_a_malformed_logical_key. |
| cfg P3-14 `--state bare.json` | **FIXED** | `cli/commands/release.rs:380` `Path::parent()` → `Some("")`, so the lock lands under the process cwd. Review-time evidence above; Fixed after the review in bd630b06 (fix(release): harden recovery, receipts, redaction, registries and package names). Tests: a_bare_state_filename_yields_the_current_directory, a_bare_relative_state_path_resolves_against_the_process_working_directory. |
| cfg P3-15 execute root vs `find_workspace_root` | **FIXED** | `cli/commands/release.rs:375` canonicalises cwd; plan uses `load_workspace`. Review-time evidence above; Fixed after the review in bd630b06 (fix(release): harden recovery, receipts, redaction, registries and package names). Test: execute_resolves_the_same_workspace_root_as_plan_from_a_subdirectory. |
| core P3 `is_safe_artifact_component` accepts `"."` | **FIXED** | `model/release.rs:1217-1224`. Harmless: `resolve_asset_path` then rejects it as not-a-regular-file. Review-time evidence above; Fixed after the review in bd630b06 (fix(release): harden recovery, receipts, redaction, registries and package names). Test: artifact_slot_components_reject_dot_and_dot_dot. |
| arch 10 manifest is unsigned | **PARTIAL** | `ArtifactManifestV1::source_commit` is now validated (`release.rs:1304-1313`, `MismatchedSourceCommit`), but the manifest itself carries no signature — the doc at `:1226-1232` states this honestly. |
| arch 12 receipt evidence vacuous | **PARTIAL** | receipt observations may still only be `Exact`. |
| arch 13 dead states | **PARTIAL** | `OperationEvent::Blocked` / `EffectFailedAndAbsent` are constructed only in tests (`release_execution.rs:545`, `model/release.rs:3217`). |
| arch 3 self-policy in generic surface | **PARTIAL** | one table now (`config/resolve.rs:151-157 PRODUCT_ARTIFACT_TARGETS`) + `RELEASE_COORDINATOR_WORKFLOW_PATH`, both still Callisto-specific. |
| wf P2-1 concurrency | **PARTIAL** | per-SHA group + `release-execute` serialization; a third queued execute still cancels the pending second (GitHub keeps 1 running + 1 pending). |
| wf P2-3 preflight ≠ release | **PARTIAL** | table agreement is now machine-enforced (policy script, 7 table mutants killed); the coordinator/source split and `artifact-manifest` are still not preflighted in CI. |
| wf P2-10 audit gates push releases | **OPEN by design** for push; **FIXED** for recovery (`recovery-checks` replaces `verify`). |
| wf P3 `plan-publish \|\| true` | **UNVERIFIED** (out of diff). |
| cfg P3-13 13 `drop(Command…output())` fixtures | **UNVERIFIED** (not re-swept). |

### FIXED (verified in current code, not from commit messages)

| ID | Where fixed |
|---|---|
| core P0-1 `cargo info` read the local workspace | Observation is now `curl --url <sparse index>` (`provider/registry.rs:301-323`, `provider/http.rs:72-87`); `cargo info` is gone from the release path. Tests: `provider/registry/tests.rs`, captured fixtures. |
| core P0-2 `gh api --repo` | `release/github.rs:38` `["api","--include","--method","GET",endpoint]`; `provider/forge.rs:266` test asserts `!args.contains("--repo")`. |
| core P0-3 / ext(2) `target_commitish` | `provider/forge.rs:187-232` never reads it; create is `--draft --verify-tag`. |
| core P1-1 / D08 tag observation local-only | `provider/tag.rs:163-218` `git ls-remote <endpoint> refs/tags/X refs/tags/X^{}`; lightweight → `Conflict::UnannotatedTag`, ls-remote failure → transient `Indeterminate` (fixes P20 too). |
| core P2-2 / C5 yanked + "already uploaded" | `provider/registry.rs:91-107` requires an exact observation regardless of client text; `:362` yanked → `RegistryVersionYanked` conflict. |
| core P2-3 / cfg-6 orchestration unbound | `ReleaseRunEnvelopeV1::validate_for_intent` (`model/release.rs:1912-1916`) requires `slot.attestation_policy.workflow_commit == orchestration_revision`. |
| core P2-4 / cfg-1 / D06 / ext(4) profile authority | `release/derive.rs:117-131`: `[release]` present ⇒ profile must resolve (E198); absent ⇒ only `production`. |
| core P2-5 forge lifecycle | draft → uploads → `release edit --draft=false` (`derive.rs:408-431`), `--prerelease` from `version.is_prerelease()`. |
| core P2-6 / cfg-11 E-code misuse | typed `CliError`/`GraphError` variants (E174 E198 …). |
| core P3 GitHubRepository case | `model/release.rs:185-207` lowercases, strips `.git`, rejects dot components. |
| cfg-2 / C4 pre-`[release]` source | `cli/commands/release.rs:232-248` emits a notice and plans zero slots; workflow gates build on `has_artifacts`. |
| cfg-4 / C8 shared-destination | `config/resolve.rs:646-698` canonical destinations, per-logical-key, lone-profile rule; 5 tests in `release_profile_isolation_tests.rs`. |
| cfg-5 / D05 / ext(1) late validation | `cli/commands/release.rs:293-335`: profile, SHA parse, `probe_atomic_write(receipt)`, manifest verification all precede `validate_release_intent`. |
| cfg-7 / C3 tag template | `previous_tag_templates` config + installer strips `callisto@` and `callisto-cli@` (`setup-callisto/action.yml:88`); `cli_tag_template.rs`. |
| cfg-9 / D07 bare product-package | `config/resolve.rs` rejects `package.ecosystem().is_none()`. |
| wf P1-1 / arch-2 orchestration = main HEAD | `callisto-release.yml:135` `orchestration_sha="$TRIGGERING_SHA"`; policy script pins it twice and the mutant is killed. |
| wf P2-5 excess credentials | only `CARGO_REGISTRY_TOKEN` in `execute`; policy asserts the exact set. |
| wf P2-6 / arch-1 receipt discarded | `:465-484` step summary + `upload-artifact` with `if: always()`. |
| wf P2-7 installer | `verify_asset()` runs `gh attestation verify --repo --signer-workflow --deny-self-hosted-runners`, fail-closed, single-member extraction, `--locked`. |
| wf P2-9 retention-days | 7 at `:248,:332,:385,:484`. |
| wf P2-2 release skipped by pending changeset | `version-pr` now runs only when `is_release_pr != 'true'`; `plan` no longer depends on it. |
| wf P3 dispatch input | 40-hex regex + `resolved_sha` equality + `head.repo.full_name` check (`:124-141`). |
| wf P3 timeouts | every job has `timeout-minutes`; policy asserts it. |
| arch-4 PyPI/NuGet regression / C2 | `release/binding.rs:87` `builtin_unconfigured`; PyPI has a real observation (`registry.rs:463-483`); `release_plan_pypi_default_registry_test.rs`. |
| arch-14/15 sibling+cohesion | `commands/release/{mod,binding,capability,derive,github,provider/*}`. |
| perf P1-1 timeouts | `provider/policy.rs:13-36` bounds every outbound command. |
| perf X1 / C1 / D04 `target/` trips trust | `recheck_trust` compares `evidence.identity()` (`vcs/access.rs:83-99`), which excludes `allowed_ignored_paths`. |
| perf P2-1 no retry | `policy.rs:88-109` bounded retry honouring `Retry-After`, clamped to 300 s. |
| perf P2-2 recovery paid full verify | `recovery-checks` job. |
| perf P3-5 / ext(3) exponential reconcile | `release_execution.rs:326,345` memoized (`satisfied`). |
| C6 manifest source_commit | `red_c6_…` in `model/tests/release_evidence_tests.rs`. |
| C7 Attempting wedge | `red_c7_…` (observe-first). |
| licence finding | **N/A** (withdrawn). |

tests §7 gaps: every D01-D08/P20/C2/C3/C6/C7/C8 red test exists and no `#[ignore]` remains anywhere in `crates/`
(`rg '#\[ignore'` → empty). Mutant **M** (PyPI Indeterminate→Absent) is closed by the real PyPI adapter. **6d**
(tag observation against a real bare remote) is still only captured-fixture based (`provider/tag.rs:285-327`).

Tests-section note from the review: every D01-D08, P20, C2, C3, C6, C7 and C8 regression test exists and no `#[ignore]` remains in `crates/`. The mutant "PyPI Indeterminate to Absent" is closed by the real PyPI adapter. Tag observation against a real bare remote (6d) is still captured-fixture based (`provider/tag.rs`).
