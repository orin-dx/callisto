# PR #101 workflow-slice audit (head 4d05e40d, merge base 5e452d04)

Method: read every file in slice; ran bash -n, shellcheck (info-only), actionlint 1.7.12 (clean with the CI's `$/` ignore), zizmor 1.29.0 (no findings at CI persona), the two hermetic contract scripts (both pass), 11 mutation runs of verify-release-workflow-contract.sh against scratch copies, and read-only `gh api` queries. No cargo run. FINDINGS.md has no workflow entries, so nothing was skipped.

## Verified facts (not inference)
- `uses: $/.github/...` is GitHub's self-repository syntax. CI run 35374825763 logged it as `orin-dx/callisto/.github/actions/callisto-validate@54831c65...`, which is the PR merge ref (`refs/pull/101/merge`). So `$/` actions load from the workflow's own SHA, NOT from the orchestration checkout.
- Only execute references secrets (release.yml:371-373). No `${{ }}` appears in any `run:` body of either workflow (parsed). All 12 external action SHAs resolve upstream; 6 tag comments checked.
- Repo secrets: none. Org secrets: CARGO_REGISTRY_TOKEN, NPM_TOKEN. TWINE_PASSWORD exists nowhere. Environment `release` exists with no rules and is no longer referenced.
- Ruleset 22292852: active, code-owner review, signed, squash-only, 12 required checks matching docs, `strict_required_status_checks_policy:false`, one owner bypass.
- CLI verifies bytes plus `gh attestation verify --signer-workflow --signer-digest <orchestration> --source-digest <orchestration> --deny-self-hosted-runners` before any effect (release_artifacts.rs:160-173) and errors when slots exist but no manifest (cli release.rs:322-334). The workflow's `if [[ -f manifest.json ]]` (release.yml:387) is therefore fail-closed.

## Findings

### P1-1 Orchestration revision is live `main` HEAD, not the workflow revision (CONFIRMED by static trace; runtime confirm = push to main during verify, or dispatch from a branch)
release.yml:128 `orchestration_sha=$(gh api .../commits/main)`, consumed at 158/226/284/334/376. `--orchestration-revision` becomes `workflow_commit` (cli release.rs:201-234), then both `--signer-digest` and `--source-digest` (release_artifacts.rs:165-170). GitHub stamps attestations with the run's own SHA (`github.sha`==`workflow_sha`).
Scenario: release merge R starts a run; verify plus version-pr take ~6 min (run 35298849697: 02:18:56 to 02:24:54); any push S to main lands; release-candidate records S; artifacts are attested as R; execute fails at `gh attestation verify`. Dispatching from any non-main ref fails deterministically (no `github.ref` guard anywhere). Fail-closed before effects, but a red release after a green preflight. Also: the coordinator that holds secrets is S, which `verify` never tested; `$/` action code is R.
Fix: `orchestration_sha="$GITHUB_SHA"` (or `GITHUB_WORKFLOW_SHA`); add `if: github.ref == 'refs/heads/main'` to release-candidate; delete the API call.

### P2-1 Pending release run can be cancelled by concurrency (release.yml:16-18) (CONFIRMED semantics per GitHub docs; not executed)
`cancel-in-progress:false` still replaces an already-pending run with the newest pending one. Run A in flight, release merge R queued, dependabot merge Z queued: R's run is cancelled; Z's run has `release_source_sha=Z`, not a release, green. Release silently unpublished until someone dispatches.
Fix: make candidate discovery independent of the triggering SHA (scan first-parent since last release tag), or per-SHA workflow group plus execute-only serialization with a detector alarm.

### P2-2 Release skipped while unrelated changeset is pending (release.yml:107-109) (CONFIRMED logic; pre-existing, retained)
Release requires `has_pending_changesets == 'false'` in the same run. R merged while C2 landed but was not folded in: version-pr sees C2, opens a new PR, release-candidate is skipped, run is green, R is never published; the next release then skips R's versions.
Fix: evaluate release-candidate first (cheap API check); run version-pr only when the commit is not a release merge.

### P2-3 Green Release Artifact Preflight does not imply green release (CONFIRMED gap)
Preflight (ci.yml:157-164) builds only workspace-as-source. It never runs the coordinator/source split (release cwd = coordinator, source = `release-source`; `rustup target add` at script:36/42/47 runs in the coordinator dir, `cargo` in the source dir; equal only because both toolchain files say `stable`), plan, attest, upload/download layout, `artifact-manifest`, or the four-slot match. The same tuple set is hand-copied: callisto.toml artifact-targets, release.rs:1254-1259 (its doc comment at 1250-1253 claims one spelling), ci.yml:129-150, release.yml:196-217, script:19-24, setup-callisto:59/65, matrix.rs:14 (says `macos-latest`; both workflows say `macos-14`). Contract test checks only `target:` lines (verify:46-49, 97-101), not `asset:`/`kind:`/`runner:` and does not compare the two matrices. Today's four asset names do match release.rs.
Fix: emit the matrix from the CLI (plan job output -> `fromJson`) or add a test asserting workflow matrices == `product_asset_name`; preflight should also run `callisto release artifact-manifest` on its outputs.

### P2-4 Contract test is mostly theatre; harness omits the workflow boundary (CONFIRMED by mutation)
All 11 mutants below still pass `verify-release-workflow-contract.sh` (control mutant `needs:[build,plan]` correctly fails): rename wasm asset; `contents: write`+`id-token: write` added to plan; `CARGO_REGISTRY_TOKEN` added to build-artifact; `persist-credentials:true` moved from release-source to coordinator checkout in execute; `environment: {name: release}` map form; `cancel-in-progress:true`; `orchestration_sha=deadbeef`; plan `if: always()`; dropped `"$verified" == true` check; manifest condition `if false`; `permissions: write-all` at top. Cause: `grep -Fqx` finds a line anywhere in an `sed` range (verify:9-20); the env-gate regex (109) matches two spellings; the injection regex (114) is a lazy multiline span over 2 of 4 composite actions. Meaningful: `needs: build` (62), action-pin script, zizmor in CI, CI checkout-vs-persist count (73-78). The specific missing checks are exactly SPEC AC[11]/incident AC3-4: no test that plan/build lack secrets or write tokens.
Harness (`durable_release_e2e_tests.rs` via run.sh) is a real CLI e2e with fake `cargo`, `git`, `gh`: `git push` always exit 0 (the PR #91 credential failure cannot recur in-test), `gh attestation verify` always exit 0 (args unasserted at this level), no credential/secret assertions (rg: none), never passes `--profile production`, never parses YAML, never evaluates `if:`/`needs`/outputs, the release-candidate jq, artifact upload LCA layout (`release-build/release-intent/...`), or permissions. Untested boundary = the YAML adapter, which is where every incident bug lived. `just release-workflow-behavior` also re-runs tests `just test-ci` already ran.
Fix: extract each `run:` body by step id and execute it against fake `gh`/`callisto` in the harness; add job-level negative checks (parsed YAML: secrets only in execute; write/id-token only where listed; `persist-credentials:true` only on `path: release-source`); generic `^\s*environment:` ban.

### P2-5 Execute holds more credentials than a crates.io release needs (CONFIRMED)
release.yml:372 NPM_TOKEN (a real org secret) and 373 TWINE_PASSWORD (undefined) are in the step that runs `cargo publish`, which builds dependency `build.rs`. Line 330 `id-token: write` is unused (rg finds no OIDC consumer). Also `build-artifact` line 238-240 installs `extractions/setup-just` (unused since `just build` was removed) inside a job with id-token+attestations write.
Fix: delete NPM_TOKEN, TWINE_PASSWORD, execute id-token, dead setup-just; add "secrets appear only in execute" to the contract.

### P2-6 Receipt and state are discarded (release.yml:381-390) (CONFIRMED)
`--state`/`--receipt` go to `${RUNNER_TEMP}/callisto-release-state/$RUN_ID` and are never uploaded, summarized, or attested, on success or failure (no `if: always()` step). Incident Phase 4 ("upload state on failure, emit receipt") and definition-of-done ("receipt records the same operations") are unmet in the adapter.
Fix: `actions/upload-artifact` with `if: always()` and `retention-days: 90`, plus a step summary.

### P2-7 setup-callisto installer: unverified binary, silent version substitution (CONFIRMED)
Now that real attested assets exist, `curl -sL -f` (action.yml:113, stderr discarded) + `tar -xzf` (123) run with no checksum/attestation/`gh attestation verify`; `latest` is mutable; extraction is not restricted to the single member `callisto`. Line 138 falls back to `cargo install --git ...` at default-branch HEAD, no `--locked`, even when the caller asked for `callisto-cli@X`. Line 108 `cargo build -p callisto-cli` lacks `--locked`. Line 106 test is `grep "callisto-cli"` in any root Cargo.toml. Linux x86_64 always picks the glibc asset (no musl). Tests stub tar/curl so nothing exercises the real archive layout. setup-callisto-wasm has the same unverified download plus a version-blind cache key.
Fix: verify with `gh attestation verify --repo --signer-workflow` before extraction; extract only `callisto`; fail instead of substituting; `--locked`.

### P2-8 Transitive mutable action in every release job (CONFIRMED)
setup-callisto:18 pins `orin-dx/actions/setup-rust@2521888`, whose action.yml uses `Swatinem/rust-cache@v2` (floating) and interpolates `${{ inputs.* }}` into shell. It runs in build-artifact (id-token) and execute (contents:write). `verify-action-pins.sh` cannot see it.
Fix: pin rust-cache by SHA inside setup-rust and re-pin here, or inline it.

### P2-9 PR base is stale: main's 7-day retention fix is not reflected (CONFIRMED)
Main has 2 commits beyond 5e452d04 (1b760014 raised `retention-days` 1->7 in this same file to allow late execute reruns). PR still sets `retention-days: 1` at release.yml:189, 270, 321. GitHub reports MERGEABLE, so a clean merge could drop the fix. Rebase and use 7.

### P2-10 Release gated on a time-varying audit (release.yml:71,107-109) (PLAUSIBLE; owner decision)
`verify` (including `just audit` against today's advisory DB) is a hard prerequisite even for recovery dispatch. This is incident F-010 again: a newly yanked/advised dev dependency blocks publishing an already merged, already reviewed release.

### P2-11 Live release PR cannot produce its own merge evidence (CONFIRMED fact; cause inferred)
PR #99 (bot-authored) has `statusCheckRollup: []`; its three latest `callisto-ci.yml` pull_request runs are `action_required` (actor github-actions[bot]; fork-approval policy `first_time_contributors`). The 12 required checks therefore need a human "Approve and run" or the owner bypass, which the docs restrict to emergencies. Expect routine bypass.

### P3 (brief)
- Dispatch input not validated (release.yml:127). Non-40-hex values pass the API check (`commits/main`), then fail at `CommitSha::parse` in plan after a wasted verify; full SHA plus `--jq .sha` equality check is cheap. Newline injection into GITHUB_OUTPUT is blocked only incidentally by the API call failing.
- release-candidate ignores `.head.repo.full_name` (134) while the release-PR action's snapshot records headRepository; a fork branch named `callisto/version-packages` matches. Check `head.repo.full_name == GITHUB_REPOSITORY`.
- build-artifact never proves `release-source` HEAD equals the intent's source (245-251 only `inspect` the intent); safe today because plan rejects non-SHA inputs.
- No `timeout-minutes` anywhere; execute can hang holding secrets. No `github.run_attempt == 1` guard against the historic-rerun mistake (F-003).
- `callisto-validate/action.yml:24` `plan-publish || true` makes the required "Validate Changesets" check blind to plan failure.
- Repo settings: `sha_pinning_required:false`, `can_approve_pull_request_reviews:true`, non-strict status checks; org secrets are readable by any branch's workflow (owner decision; do not add an Environment gate).
- Reproducibility: toolchain `stable`, unpinned `taiki-e` tools (cargo-deny, nextest), `ubuntu-latest`, no `SOURCE_DATE_EPOCH`, tar with owner/mtime, no separate checksum file, no timeouts.
- Repeated hardcodes: `just-version: '1.58.0'` x7, `orin-dx/callisto` x5, `callisto/version-packages` in jq/step/action, workflow path `.github/workflows/callisto-release.yml` at cli release.rs:233, profile `production` x2, `cross 0.2.5` in script and test, artifact/dir names in 3 jobs plus the contract.
- Build-script test's negative case (test:72-81) passes on any failure, not the allowlist rejection.
- `.claude/settings.json` deny rule removal is intentional (34c3e37a); the maintainer-tool push setting left disabled is good.

## Done well
1. Isolated checkouts, `persist-credentials:false` except release-source in execute, everything under RUNNER_TEMP.
2. Every untrusted or derived value enters through `env:`; zizmor strict and actionlint clean; SHA pins verified upstream; CODEOWNERS plus code-owner ruleset.
3. Attestation and byte verification owned by the CLI, before any effect, symlink and path-escape rejection, fail-closed on manifest/slot mismatch.
4. Build script: tuple allowlist, `--locked`, pinned `cross`, required env with `:?`; real four-target preflight is green on the PR head.
5. Recovery separates orchestration from source, drops the Environment gate, and keeps merge-plus-verified-signature as the only trigger (workflow_dispatch cannot publish an unmerged SHA: API requires a merged PR into main from the managed head branch and a verified commit).
