# PR #101 audit: config + CLI slice (read-only; no cargo run)

Head 4d05e40d, base 5e452d04. FINDINGS.md had no entries for this PR. Line numbers are at head.
Verified = traced in code. Inference = labelled.

## P1

### 1. Unconfigured profile fails OPEN to the un-routed production registry when `[release]` is absent (docs/07:50 says it fails before an intent is written)
Status: CONFIRMED (traced, not executed).
- crates/callisto-cli/src/commands/release.rs:243-250: `(None,None,None)` branch calls `build_release_intent(..., profile, ...)` with no profile-configured check. The only check ("not configured with a forge destination", :210-215) lives in the `(Some,Some,Some)` branch.
- release.rs:340: execute only checks the profile `if source_workspace.config.product_release.is_some()`.
- crates/callisto-graph/src/commands/release.rs:1312-1316 collapses "no [release]" and "profile missing" into `Option::None`; :1801-1815 then does `.unwrap_or_else(|| logical_key.clone())`, i.e. the production key. Validation (:361-368) re-derives with the same fallback, so `expected == received`.
- Only test is `[release]`-present (durable_release_e2e_tests.rs:424-457).
Failing input: workspace with no `[release]`; `callisto release plan --profile rehearsal --from-release-commit <sha> --decision ... --out i.json`, then `release execute --profile rehearsal --intent i.json ...`. Intent is written, profile check at :302 passes (matches intent), :340 is skipped, crates.io publish runs with CARGO_REGISTRY_TOKEN, receipt says "rehearsal".
Fix: enforce in graph `derive_release_inputs` (single owner, SRL-05): if `[release]` exists the profile must resolve or error; if absent, accept only the default profile id (one model constant). Add a CLI test for no-`[release]` + non-default profile.

### 2. The workflow cannot recover any source that predates `[release]`, including the incident's named source 0e0016ed (0.7.2)
Status: CONFIRMED mechanism; impact depends on which source is recovered.
- `git show 0e0016ed:callisto.toml` has no `[release]` (verified).
- .github/workflows/callisto-release.yml:178-179 always passes `--orchestration-revision` and `--artifact-repository`; CLI release.rs:251-255 rejects those flags when the source has no product release. Downstream, `artifact-manifest` errors on zero slots (release.rs:51-55) and the build/manifest steps are unconditional.
- Incident plan Phase 1 step 4 ("choose the exact source commit") allows a post-PR repair commit, which would pass; recovering 0e0016ed as-is would not (SRL-04's headline scenario is only proven with `[release]` fixtures).
Fix: decide explicitly. Either detect "source has no [release]" in release-candidate and fail with an actionable message, or let plan ignore the flags and make build/manifest conditional on slot count.

### 3. AGENTS.md/CLAUDE.md now assert FSL-1.1-MIT/"MIT"; every manifest and LICENSE still say otherwise
Status: CONFIRMED (grep).
AGENTS.md:17-38 and CLAUDE.md:20 claim "actual Cargo.toml license field" is FSL-1.1-MIT (product) / MIT (model, format, vcs). Actual: `AGPL-3.0-only` for cli/graph/manifests/conventional/changelog/moon/fixtures; `MIT OR Apache-2.0` for model/format/vcs; LICENSE is AGPL-3.0; deny.toml allow-list has AGPL, no FSL. CONTRIBUTING.md:89-90, README.md:276, ARCHITECTURE.md:21,122-129 (same PR touched CONTRIBUTING/README) still say AGPL. `git log -S FSL-1.1-MIT` finds only doc commit 37e1ac68.
Impact: crates.io license metadata is immutable per version; if FSL is intended, manifests must change before the first publish. Fix: land manifests/LICENSE/deny.toml/README/CONTRIBUTING/ARCHITECTURE together, or revert the docs.

## P2

### 4. Shared-destination isolation is bypassable by spelling, has a false positive, and is only pairwise (resolve.rs:180-194, 615-640)
Status: CONFIRMED by reading both code paths.
- Registry: config compares raw `registry.url` strings (:623-626). Graph canonicalises (crates/callisto-graph/src/commands/release.rs:1925-1930: lowercased host, default port, path verbatim). Two keys with `https://Registry.Example.test:443/index` and `https://registry.example.test/index` pass config and produce identical binding digests. `/index` vs `/index/` also evades.
- False positive: `Option<&str>` equality makes `None == None` true. production `{cratesIo="cratesIo"}` + other profile `{npm="npm"}` (both built-ins, url None) is rejected as "share registry destination `cratesIo`". Because `load()` runs for every command, this breaks all callisto commands in that repo.
- Forge: `GitHubRepository` (callisto-model release.rs:167-169, 262-269) derives Eq on raw strings: `orin-dx/callisto` vs `Orin-DX/callisto` or `orin-dx/callisto.git` are "different"; the git-remote path strips `.git` (graph release.rs:2004) but config parse does not. `.`/`..` parts also parse.
- Relational only: a lone non-production profile may route `cratesIo -> cratesIo` (real crates.io) with no error, contrary to SRL-11 / docs/07:56.
- Sibling copies: legacy publish.rs:118-120 raw-string compare (https-only). Three URL-equivalence implementations exist.
Fix: one `canonical_registry_endpoint` (graph) used by config validation and release; compare digests, same-key only for None-url; canonicalise GitHubRepository (ASCII case, strip `.git`, reject `.`/`..`).

### 5. `release execute` validates its own inputs after irreversible effects (cli release.rs:376-402)
Status: CONFIRMED.
`execute_release_with_artifacts_in_recovery` runs at :382; `--orchestration-revision` is parsed at :389-394 and the Hermetic-source check at :395-402 come after. Failing input: `--orchestration-revision abc123` publishes, tags, then exits 1 with no receipt. Also `--dry-run` is rejected at :376, after `verify_artifact_manifest` (:328) shelled `gh attestation verify`. Recoverable by rerun (provider observation) but avoidable. Fix: parse/validate all args first.

### 6. Execute never cross-checks `--orchestration-revision` against the intent's attestation `workflow_commit`
Status: CONFIRMED absence (rg over crates); receipt provenance is then a caller claim. release.rs:403-413 uses the arg; slots carry `attestation_policy.workflow_commit` (verified by `gh attestation verify --signer-digest`, release_artifacts.rs:165-170). Fix: with slots, require equality. Cross-slice, PLAUSIBLE: workflow :128 derives orchestration_sha from `gh api commits/main`, not `github.sha`; a race or non-main dispatch makes signer-digest verification fail closed with an opaque message.

### 7. callisto.toml:44-46 renames the CLI tag template to `callisto@{version}`; consumers are unaware
Status: (b) CONFIRMED, (a) PLAUSIBLE.
(a) Existing tags are `callisto-cli@0.4.0..0.5.0`; glob `callisto@*` will not match them, so last-tag lookup for cargo/callisto-cli is empty (confirm: `callisto status --format json`, expect null lastTag).
(b) .github/actions/setup-callisto/action.yml:83-85 strips only `callisto-cli@`; for `callisto@0.7.2` the crates.io fallback (:133) gets a bogus version and falls through to `cargo install --git ...` (:138), an UNPINNED main install, on any platform with no prebuilt asset. Test at setup-callisto/tests covers only the old prefix.

### 8. Hardcoded values (dedicated pass)
- Target list: resolve.rs:157-162 `TARGETS` duplicates callisto.toml:22-27, graph `product_asset_name` (release.rs:1254-1262), workflow matrix, scripts, tests. The config key is validate-only (:197 overwrites with the constant), so it is not configurable. Drift between resolve.rs and `product_asset_name` panics at graph release.rs:1552 `.expect("validated product target")`.
- Asset names `callisto-*.tar.gz` / `callisto-moon.wasm` and workflow path `.github/workflows/callisto-release.yml` (cli release.rs:233) are baked into shared crates; any other adopter's `[release]` yields Callisto-named assets and the wrong signer workflow.
- `"production"` default at cli.rs:329 and :429 plus 10+ `ReleaseProfileId::parse("production")` sites; no model constant.
- resolve.rs:600-601 exempts only `cratesIo`/`npm` from needing a URL although `RegistryKey::PYPI/NUGET` exist (graph accepts key-only bindings, release.rs:1844-1850), so PyPI cannot be routed without a URL.
- E197 help (error.rs:793-797) hardcodes "all four required artifact targets".
Fix: one `PRODUCT_ARTIFACT_TARGETS`/asset table in one crate; make `--profile` required or use a constant.

### 9. `product-package` validation gaps (resolve.rs:153-156)
Status: CONFIRMED for bare id. Message says "ecosystem-qualified" but `PackageId::parse("callisto-cli")` returns Bare (identity.rs:58) and is accepted. graph release.rs:1537 (`product.package.ecosystem() != Some(...)`) then silently derives zero slots, surfacing later as "declares no binary artifact slots". Also unchecked: ecosystem supported, package exists, has `github-release` in publish-to (else non-actionable ReleaseInvariant at graph :1540-1542). Fix: require `.ecosystem().is_some()` and cross-validate against `[[package]]`.

## P3
- 10. Config vs plan-time gaps: route logical keys unvalidated (`cratesio` typo passes load, fails at plan with "no registry route", graph :1805-1811); config error says "without a credential-free URL" (resolve.rs:604-611) but only checks `url.is_none()`; userinfo/query/fragment are rejected only at plan for the selected profile (graph :1907-1924); ecosystem-kind compat only at :1827; `http://` allowed (:1901) so publish tokens can go cleartext and http/https are distinct. Errors do not echo the URL (good); legacy `UntrustedNpmRegistry{url}` (publish.rs:119-125) echoes package.json userinfo (pre-existing sibling).
- 11. Error UX: `CliError::Other` for ~20 release failures (single code, no help); "not configured with a forge destination" (cli release.rs:212, 348) does not name `[release.profiles.<name>]`; resolve.rs:154 drops the parse reason (`|_error|`); artifact-targets error (:171) lists neither expected set nor offender; main.rs exit-code contract omits release/release-pr/filter-plan.
- 12. Stale docs: cli release.rs:1-7 says execution "is intentionally not wired"; docs/06:33,49 future tense vs implemented; docs/07 repeats the rehearsal-repo paragraph and says profiles "declare" registry-routes though optional (raw.rs:49); docs/01-spec §14 and .claude/semantic-model/config-resolution.md lack `[release]`.
- 13. Sibling sweep (CLAUDE.md rule 1): the PR hardened snapshot.rs fixtures only; 13 identical swallow-failure fixtures remain (`drop(Command...output())`): compose_pr_body.rs:57, tag.rs:49,98, strict_flag_tests.rs x7, e2e_tests.rs x3. No deferral recorded.
- 14. `--state bare.json`: `Path::parent()` is `Some("")` (cli release.rs:369), so the lock lands under process cwd (vcs release_lock.rs:90-96). PLAUSIBLE.
- 15. `execute` uses canonicalised cwd as root (:364) while plan uses `find_workspace_root` (workspace.rs:29); `--source-root <subdir>` diverges. Pre-existing pattern extended.
- 16. `--recovery` is a label; effective only when local state is missing (graph release_execution.rs:69); no check source != orchestration (legitimate either way).
- tty.rs: VERIFIED no regression. Only the environment-dependent test was deleted; `is_interactive` (tty.rs:11-14) unchanged; consumers add.rs:57, init.rs:19; non-TTY add covered by cli_tests.rs:43. No test covers init's TTY branch (already the case).
- RUNNER_TEMP: CLI never reads it; all paths explicit; default state uses platform_state_directory (release_store.rs:53). No env dependency.
- unwrap/expect: allowed by workspace lint (Cargo.toml:60-61); cli release.rs:144 expect is guarded by clap `requires` (cli.rs:356).

## Done well
1. deny_unknown_fields on new raw structs; ReleaseProfileId charset/length validated; CommitSha strictly 40-hex (no abbreviations).
2. Credential-free design: intent stores key plus digest only; UnsafeRegistryBinding never echoes the URL; userinfo/query/fragment/non-http rejected at plan.
3. Fail-closed flag combinations: plan package/from-release-commit conflict, `requires = decision`, artifact flags all-or-nothing (release.rs:314-336), profile-vs-intent check, dry-run rejected up front for plan/manifest.
4. All outputs via atomic_write; receipt only after terminal observation; manifest builder rejects symlinks/non-regular/escaping paths (:56-89).
5. Deterministic BTreeMap-based, tested pairwise isolation check at load; fixture hardening in snapshot.rs.
