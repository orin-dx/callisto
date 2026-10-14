# V1 implementation evidence

## Verification rerun (2026-09-28)

- Formatting, workspace Clippy, all 1,861 Nextest tests, doctests, warning-free docs, zizmor, workflow/action contracts, the release-workflow behavior suite (25/25), and the 90% coverage gate passed. `just coverage 90` reported 93.44% line coverage.
- Native pnpm/uv tests passed with `UV_CACHE_DIR=/private/tmp/callisto-uv-cache`. The Cargo `file://` registry tests now use a fixture-local `CARGO_HOME` and all 11 matching registry cases pass without network access.
- `cargo deny --offline check` passed with access to its existing advisory database. The aggregate `just ci` invocation still cannot complete inside the workspace sandbox because cargo-deny cannot create its advisory lock under `/Users/gabe/.cargo/advisory-dbs`; no code or repository config change is needed for that local permission boundary.
- Focused corrections passed: npm disjunction parsing/coverage, direct-child changeset path recognition, Cargo-only artifact-builder detection, native-client credential routing, Python build credential filtering, and pnpm frozen-lockfile refresh. npm's canonical `node-semver` docs confirm a partial `1.2` is an X-range (`1.2.x`), not a tilde range; the normal-manifest and workspace tests cover its upper bound.
- After the npm partial-range correction, `cargo nextest run --workspace` again passed all 1,861 tests (one test process was reported as leaky by nextest, with no failure), workspace Clippy passed, and `just coverage 90` again reported 93.44%. `actionlint` and the real-binary release-PR contract also passed.
- JSON parsing and `git diff --check` passed. Full CI evidence is component-wise; do not report the sandboxed aggregate `just ci` process itself as green.
- After adding the prerelease-exit recovery regression, `UV_CACHE_DIR=/private/tmp/callisto-uv-cache rtk proxy cargo nextest run --workspace` passed all 1,862 tests, workspace Clippy passed, and `UV_CACHE_DIR=/private/tmp/callisto-uv-cache rtk proxy just coverage 90` passed at 93.45% line coverage. The first no-override Nextest attempt stopped on two uv-cache permission failures; the full rerun passed. Formatting, docs and whitespace checks passed after the final assertion changes.

## Fresh-repository onboarding smoke (2026-09-28)

- In isolated Cargo, npm and PyPI repositories, the current CLI completed `init --yes --versioning independent --no-workflow`, `add`, `status`, committed the input, `version`, committed the generated version, and `release --dry-run`. Each dry-run selected the expected `0.1.1` registry publication and tag. No credentials, registry publication, GitHub release or tag push was attempted.
- The npm fixture passed `npm pack --dry-run`. The Cargo fixture passed `cargo package --list` (with Cargo's expected missing package-metadata warning). Python version planning and release dry-run passed, but `uv build` could not obtain `setuptools` because the sandbox has no network; built wheel/sdist metadata and consumer installation remain unverified.
- The first Cargo fixture lacked a Rust target, so `version` wrote its version output before Cargo lock refresh failed with E208. The follow-up invocation refused with E121 and manual recovery guidance. After adding the missing target and committing the retained output, the dry-run succeeded. This exercises the fail-closed recovery boundary, not an uninterrupted first-run path.
- These are functional smoke checks, not timed studies. They do not establish the under-ten-minute first-published-release goal. A hosted release and real credentials/index behavior remain unverified and require separate authorization.

## Current-contract batch: T01 and T11

Baseline: `8ef9aa5a4` (#168), dirty shared checkout, 2026-09-27. Existing documentation and plan edits were retained. This batch is local implementation, not a merged release or approval to publish.

Smith coordinated two bounded shell workers, the coordinator-owned Rust change and a separate read-only reviewer. Cargo checks ran sequentially. Shared specs, operator docs and changesets remained coordinator-owned. The batch changes REL-PR-02 and REL-PR-06; REL-PR-04/07 producer/consumer behavior is exercised without changing the action's forge policy or output envelope.

| Task | Implementation | Regression evidence |
| --- | --- | --- |
| T01 | CLI-only decision input adapter accepts bare wire or decide report; strict metadata, unknown/duplicate rejection and shared domain validation remain intact. Post-update checking permits only the exact selected PR head transition. No wire schema change or action-script JSON stripping. | Actual unchanged decide stdout failed E281 before correction despite four passing CLI tests. Eight CLI unit tests and two new model tests pass. The real-binary suite passes 36 cases across bare/wrapped input transports, applied transitions and malformed/stale/domain negatives. The shell action harness passes 21 checks, including a changing forge head and final-check failure handling. |
| T11 | Auto honors explicit/action-ref pins; local requires explicit selection and verified source ownership. Both actions share the same installer. Builds reuse the restored checkout target cache. | New installer fixtures failed ten cases against the old script. Corrected action-ref/source tests pass 17 cases, crates.io fallback six and download/attestation 20. These tests fake native tools but use real jq. Real Git/Cargo/jq ownership checks also pass with empty dependency caches under Cargo 1.96 and 1.98; build/copy were simulated for those cold-cache checks. |

Two patch changesets target `callisto-cli`: `vivid-bears-hunt` for release-PR verification and `calm-lions-surge` for action installation. The release-PR topic is committed locally as `20ce30cd1`, with its unchanged-head follow-up in `f4ce5f22d`; the installer topic is `a37025927`. The later model/release batch and its supporting documentation remain uncommitted. No push, PR, merge or publication was performed.

## Verification status

- `rtk proxy just ci` passed on the final implementation with normal native cache access: 1,738 Nextest cases, doctests, formatting, workspace Clippy, dependency audit, warning-free docs, workflow security/contract/policy checks, 14 durable release-behavior cases and the 90% coverage gate. Final line coverage: 92.46%.
- `rtk proxy bash .github/actions/callisto-action/tests/test_release_pr_decide_binary_contract.sh` passed all 36 checks; it builds and calls the real binary. Commit-plan integration tests also passed within full CI.
- The shell action harness passed both normally and with CI's minimal PATH. Against the original script and a wrong-SHA variant, the applied-head fixture failed and prevented PR writes. Combined with the original E281 and installer red fixtures, this is targeted fault evidence, not a broad mutation campaign.
- Shell syntax, whitespace, JSON syntax/unique-ID and plan dependency/current-criterion mapping checks passed. No full external JSON Schema validator was available for the plugin artifacts; required fields and selected contracts were inspected directly.
- Two sandboxed `just ci` attempts stopped at native cache access: uv's default cache was unwritable; redirecting that cache exposed Cargo registry fixture failures. No tests were disabled. Both normal-cache-access CI runs passed; the final rerun includes the review corrections.
- Local actionlint passed with the same `$/` self-repository syntax exception used by CI. An unconfigured actionlint invocation rejected that existing syntax.
- Independent review found the post-ref-move E148 failure and unnecessary cache bypass. Both were corrected. A separate final reviewer found no reachable must-fix issue in the bounded delta. Pre-existing documentation changes were excluded from that implementation verdict.
- The real local installer subsequently built from this checkout into its cached target directory, copied the binary into an isolated runner-temporary directory and executed it successfully (`callisto 0.8.0`). No downloaded release or fake Cargo build was used for this final check.

## Scoped gate

Pass for T01/T11's local implementation against REL-PR-02/04/06/07, not all release behavior or the 24-task portfolio. Full CI, targeted boundary/fault checks, the real local installer and independent review are complete. Hosted checks remain outside this local verdict; remote PR handoff and publication are not authorized by it.

## Boundaries

Hosted Linux/Windows behavior, downloaded releases, live registry/forge publication and release-PR refresh are not established by local fixture tests. Linux-gated real-registry E2Es ran zero cases on this Mac; existing hosted registry and platform artifact-preflight checks remain CI evidence. This batch does not fix tagged local retries, prerelease Python baseline authority, D01 ownership/migration decisions or the other open findings.

Before a PR handoff, inspect each topic's delta separately from the pre-existing documentation work. Before merging a refreshed release PR, verify its current source/decision agreement and required CI. Do not merge stale #113 as a workaround. Model-changing batches still need the D01 contracts in the execution guide.

## Parallel current-contract batch: T02, T03 and T04

Execution started on 2026-09-27 from the same dirty checkout after T01. All changes in this batch remain uncommitted. No remote action was taken.

| Writer | Exclusive scope | Status |
| --- | --- | --- |
| Schemas | String-shaped schemas for custom wire types and complete payload validation across every schema target | Implemented; 13 schema checks pass; full CI passes |
| Durable versions | Distribution-context decoding for release decision, operations, slots and provider evidence | Implemented; model and durable Python release tests pass; independent scoped review found no must-fix |
| Native dependencies | Declaration keys/aliases, native target names, inherited Cargo root routing, ecosystem-aware cascade/floor/snapshot rewriting, and PEP 503 Python lookup | Implemented; full graph suite and native CLI suite pass; independent T04 review cleared both findings |

Smith assigned disjoint file ownership. The coordinator owns specs, docs, changesets, dependency declarations, the lockfile, integration, independent review and one serial Cargo verification queue. Writers did not stage files or commit. Shared release-owner, migration and output contracts are not implicitly approved by these repairs.

The dev-only full-schema validator is `jsonschema` 0.53 with default features disabled. Offline dependency resolution succeeded; the lockfile adds its dependency closure without refreshing existing package versions. `cargo test -p callisto-model` passed 362 unit, five integration and one doc test; all 19 durable release E2E tests passed; all 729 graph unit tests and graph integration tests passed with normal cache access; and all five native-resolution E2E tests passed with normal cache access. The complete `just ci` pipeline passed 1,787 Nextest cases, workspace Clippy, workflow checks and the 90% coverage gate, with 93.54% line coverage. Sandboxed graph and uv runs failed only where native cache access was blocked; reruns with normal cache access passed. `callisto add` could not read the local ignored `callisto.toml` because its pre-existing `[release]` table is unsupported by this checkout, so the three standard patch changeset files were added directly without changing that config.

## Follow-on model batch: T02, T03, T04, T03-PRE, T03-HISTORY and T04-PNPM

This batch remains local and uncommitted. The pre-existing documentation reconciliation and T11 installer work are preserved in the same shared checkout; this verification covers only the named model/release tasks and their current-contract spec updates.

- `rtk proxy just ci` passed with normal uv-cache access: formatting, workspace Clippy, all 1,822 Nextest tests, workflow contract/policy and security checks, docs, doctests and the 90% line-coverage gate. Final line coverage: 93.53%.
- `callisto-model` passed 368 unit tests, five release-transition integration tests, eight release-evidence tests and its doc test. `callisto-manifests` passed 210 unit tests, three Cargo integration tests and four persistence tests. The graph suite passed all 731 unit tests and its integration suites within full CI.
- Focused evidence: pre-mode tests 17/17; durable release E2E 25/25; partial-run tests 3/3; single-package root tests 4/4; pnpm protocol/version/snapshot/native-pack tests passed, including the full nine-case native-resolution suite under full CI. The post-CI raw-baseline compatibility correction passed pre-mode tests and workspace Clippy.
- Independent T04 review's Python PEP 503 and Cargo inherited-alias findings were corrected and re-reviewed with no must-fix. T03-PRE review's lost original SemVer spelling was fixed and covered by `v1.2.3` and whitespace-padded regressions; BOM-prefixed `pre.json` is also preserved through exit. T03-HISTORY and T04-PNPM still need independent scoped review.
- The first sandboxed full-CI attempt was blocked by uv cache permissions and then by the architecture guard's coarse manifest-parser heuristic. With normal cache access, after adding a reasoned allowlist entry for the release-decision JSON decoder (historical manifest parsing itself uses `callisto_manifests::read_effective_version`), the complete rerun passed. No tests were disabled.
- Three follow-on patch changesets were added for native pre-release baselines, pnpm workspace ranges and historical release authority. The existing ignored local `callisto.toml` still prevents `callisto add` from running because its pre-existing `[release]` table is unsupported by this checkout; it was not changed.

At this checkpoint, the preceding batches were local verification results, not a merge or release; T04-LOCK, T05 and the remaining v1 stacks were still open. The current status is below.

## Combined implementation state after T04-LOCK, T05 and T10-BINS

The local `work/v1-implementation-stack` branch is based on the fetched `origin/main` at `0a971b211` (#172), with the unique T01 commit replayed after upstream #169/#170. The former local `main` tip is preserved by `backup/local-main-before-origin-sync-2026-09-27`; the dirty implementation snapshot is also retained in `stash@{0}`. Local `main` now exactly matches `origin/main`. No push or PR was made.

- `rtk proxy cargo fmt && rtk proxy just ci` passed on the pre-sync combined worktree: formatting, workspace Clippy, all 1,841 Nextest tests, documentation/workflow/security checks, doctests and the 90% coverage gate. Line coverage was 93.46%.
- T05 targeted evidence passed: graph tests (737 unit tests plus integration suites); partial-run (5/5), durable-release (25/25), dry-run invariant (10/10) and prerelease (17/17) CLI suites. Fault cases cover a lock refresh failure after manifest write, a staging failure after manifest write, changeset-removal staging failure after generated-state staging, an untracked input, decision-write failure and prerelease reruns.
- After rebasing T01 onto current upstream #170, `cargo test -p callisto-model snapshot_after_update`, `cargo test -p callisto-cli decision_reader`, `test_release_pr_contract.sh` and `test_release_pr_decide_binary_contract.sh` passed. The resolver preserves strict validation of decide-report metadata while retaining T01's exact applied-head transition check.
- The project status/findings documents were updated after the full CI run. Rerun the lightweight documentation/schema checks or full `just ci` before PR. T03-HISTORY, T04-PNPM and T05 still need independent scoped review; the T01 rebased change also needs review as a standalone PR delta.

This is local evidence only. No work after T01 has been committed, pushed or opened as a PR. The backup branch and stash are intentionally retained until the restored worktree and resulting PR stack are reviewed.

## Current local state after T10-AUTH

The release-auth task is implemented on `work/v1-implementation-stack`, based on `origin/main` `0a971b211`; local `main` remains exactly aligned with the remote-tracking branch. The npm/Twine runner contract, generated simple/matrix workflow credential-scope checks, release-workflow policy and user-facing changeset are present. The shared `run-release.sh` test proves a fresh-home publisher sees npm/Twine native credentials, the existing `.npmrc` is unchanged, and the temporary auth config is removed. The release process and native npm publish hooks retain package-owner-defined credential visibility; Python build subprocesses have registry credentials removed.

- `rtk proxy cargo fmt && rtk proxy env UV_CACHE_DIR=/private/tmp/callisto-uv-cache-v1 just ci` passed: formatting, workspace Clippy, all 1,844 Nextest tests, workflow and shell contracts, docs/doctests, release-workflow behavior tests and the 90% coverage gate. Line coverage: 93.44%.
- Targeted auth checks passed: `cargo test -p callisto-graph render_workflow_wires_a_secret_per_detected_ecosystem`, `test_release_contract.sh` and `verify-release-workflow-policy.sh`. The generated matrix workflow checks also ran in full CI.
- One unredirected sandbox run failed because uv attempted to access the protected user cache. The selected uv end-to-end test passed with `UV_CACHE_DIR` under writable temporary storage; no tests were disabled. The full run then passed with that same cache setting and network enabled for real crates.io observation tests.
- The initial pre-format full run exposed only rustfmt differences in the added workflow test; `cargo fmt` corrected them before the passing run.

At this earlier checkpoint, all work after T01 was uncommitted, unpushed and unopened as a PR, and independent review remained for T03-HISTORY, T04-PNPM, T05, T10-AUTH and rebased T01. The v1 tasks beyond the locally implemented batches remain open; completing these changes is not equivalent to v1 readiness.

## Review-correction verification (2026-09-27)

- `rtk proxy env UV_CACHE_DIR=/private/tmp/callisto-uv-cache-v1 just ci` passed after the T01, T05 and T10 follow-up changes: 1,851 Nextest cases and 93.41% total line coverage, above the 90% gate. Formatting, workspace Clippy, docs, release-workflow, shell contracts, dependency checks and workflow policy are included in that recipe.
- `rtk proxy cargo test -p callisto-model snapshot_after_update_accepts_only_the_applied_head_transition` passed. The assertion supplies the selected PR's unchanged original head, so removing the guard makes the test fail.
- `rtk proxy cargo test -p callisto-cli --test partial_run_tests` passed all 8 cases, including stable-version refusal during `mode: exit` and staged inherited Cargo metadata when the workspace root is unstaged. `rtk proxy cargo test -p callisto-cli --test pre_mode_tests` passed all 18 cases, including legitimate stable finalization for Cargo, npm and Python prerelease cycles and the injected failed-pre-state-deletion/fresh-E121 path.
- `rtk proxy cargo test -p callisto-graph python_build_removes_every_release_credential_from_its_environment` and `.github/actions/callisto-action/tests/test_release_contract.sh` passed. The temporary npm config contains a literal `${NPM_TOKEN}` reference rather than the secret; npm resolves it at publish time, while Python build children have `NPM_TOKEN`, `NODE_AUTH_TOKEN`, `NPM_CONFIG_USERCONFIG`, and other release credentials removed.
- `rtk proxy actionlint -color '-ignore=\$/.+'` passed with local actionlint 1.7.12. The real-binary `test_release_pr_decide_binary_contract.sh` passed every create/update/noop, transport, malformed-input and applied-head case. The production artifact script built the `aarch64-apple-darwin` release binary and archive; the archive contains the expected arm64 Mach-O `callisto` executable. Linux GNU/musl targets remain hosted checks.
- Independent follow-up review cleared T01 and the examined T05 exit-mode/inherited-Cargo cases. The failed-`git rm --cached pre.json` boundary now has an end-to-end regression: stable outputs are staged, the pre-state remains, and a fresh `version` run refuses with E121 and manual recovery guidance. This is the selected fail-closed policy, not automatic retry. T10 review found no interpolation regression or Python-build file leak. Native publish hooks retain their normal owner-defined environment, and Callisto does not suppress or rewrite them.
- A full run before the prerelease-exit correction exposed three failures in legitimate finalization tests. The correction permits prerelease versions awaiting stable finalization while refusing a changed stable version on retry; the subsequent full `just ci` passed.

T11 is an independently reviewed local commit (`a37025927`) with only its six installer files. T01's unchanged-head guard is independently reviewed and committed as `f4ce5f22d`, following the main T01 commit `20ce30cd1`. A local `pr/v1-current-contract` branch points at the three commits; it has not been pushed or opened remotely. The remaining model/release implementation and docs are unstaged in the shared worktree. Hosted PR checks, fresh release-PR regeneration, release publication and fresh-repository onboarding timing have not been performed.
