# PR #101 core-slice audit (head 4d05e40d)

Slice: callisto-model release.rs/tag.rs; callisto-graph commands/{release,release_execution,release_artifacts,release_store,publish,registry_argv,mod}.rs, error.rs.
Method: static read plus three read-only probes of real tools (cargo info in a scratch dir and in the repo, gh api GET, gh --help). No build/test/clippy. Repo left clean.

## Verified vs inferred
Verified by live probe: (a) `cargo info <member>@<unpublished-version>` inside a workspace exits 0 from the local manifest; with `--registry crates-io` exits 101. (b) `gh api ... --repo X` exits 1 "unknown flag: --repo" (gh 2.101.0). (c) Real releases created without --target report target_commitish "main" (orin-dx/callisto releases API), `gh release create` documents --target default = main branch. (d) `cargo info` on a yanked version (chacha20@0.10.1) says "could not find".
Inferred: everything marked PLAUSIBLE.

## P0 (release-blocking)

### P0-1 registry observation reads local workspace, not the registry -- CONFIRMED
- release.rs:781-799 `cargo_version_is_published` runs `cargo info name@ver` with cwd=`self.prepared.root` (l.794); `--registry` only when key != CRATES_IO (l.487,564,656). callisto.toml:31 routes cratesIo->cratesIo, so no flag is ever passed. Used by observe (486-495), dispatch precheck (565), post-publish confirm poll (655-660).
- Trace, normal mode: execute:78-81 persists Attempting, dispatch_registry:565 sees "published" -> Err(ReleaseRegistryVersionExists); nothing publishes. Operator then runs `--recovery`: reconstruct_missing_state (release_execution.rs:97-117) observes Exact for all nine crates -> AlreadySatisfied with zero publishes; tags for unpublished crates get pushed; receipt observation (release.rs:252-268) also returns Exact -> forged terminal receipt. This is the incident's class (state says done, provider says not).
- Why tests miss it: E2E fake `cargo info` (durable_release_e2e_tests.rs:506) returns "could not find" until publish.
- Fix: run from a neutral temp cwd outside any workspace and pass an explicit registry (built-in name is `crates-io`, not `cratesIo`); add a real-binary contract test; make fakes reject unknown flags.

### P0-2 `gh api --repo` is not a flag; every forge/artifact observation errors -- CONFIRMED (live)
- release.rs:1108-1118 (observed_forge_release_target) and 997-1006 (observe_artifact_upload, new in PR, copy-paste) pass `"--repo"` to `gh api`. Real gh exits 1 with empty stdout; github_api_response (1199-1216) returns Err(ReleaseCommand). observe_prepared(ForgeRelease/ArtifactUpload) therefore always Errs; reconstruct (execute:113 `?`) aborts recovery entirely; dispatch_forge_release/artifact and receipt observation fail. The product forge release (callisto.toml:45) and 4 asset uploads cannot succeed.
- Fix: drop `--repo`; endpoint already carries owner/repo. Unify the two duplicated GETs into one `fetch_github_release(repo, tag)` helper (sibling rule).

### P0-3 forge "exact" compares target_commitish to a SHA; GitHub returns the branch name -- CONFIRMED by live API + code, unreachable today only because of P0-2
- release.rs:1144-1145 requires `target_commitish == source SHA`; dispatch_forge_release:879-887 runs `gh release create <tag> --repo --verify-tag --generate-notes` with no `--target`. Live: orin-dx/callisto releases show "main". After a successful create, l.899 sees Conflict -> ReleaseRemoteConflict(ForgeReleaseNotObservedAfterCreate); op stays Attempting; next recovery observes Conflict -> ReleaseRecoveryUnresolved (execute:134-139) forever, after crates/tags are irreversibly out. Artifact uploads (depend on forge Exact, l.938-950) never run.
- Fix: pass `--target <sha>` on create and/or verify the tag->commit binding via the tag op instead of target_commitish. Fake gh in harness hardcodes target_commitish=SHA (e2e test l.518).

## P1

### P1-1 Tag "provider observation" is local refs only; post-push check is tautological -- CONFIRMED
- observed_tag (release.rs:1048-1100) is `git rev-parse refs/tags/X` on the local repo; no ls-remote anywhere (grep). dispatch_tag creates the local tag (824-830), THEN checks remote validity (831), pushes (833), and "confirms" with the same local lookup (844-853). Eight of nine crates have tag but no forge op, so tag evidence is the only tag proof.
- Sequence: create local tag, push fails (network/auth/rejected) -> state Attempting persisted -> rerun with same state/checkout: recover_interrupted (execute:170-184) observes local tag Exact -> AlreadySatisfied; receipt observation (also local) Exact -> receipt with no remote tag. Workflow is masked because fresh runner + fetch-depth 0, but the CLI contract (`--state`) is exposed.
- Fix: observe via `git ls-remote <endpoint> refs/tags/X refs/tags/X^{}`; validate remote before creating tag; classify exit>1 as Indeterminate. Also l.1052-1054 treats any non-zero rev-parse (exit 128 = repo error) as Absent, and l.1084-1092 turns lightweight/foreign tags into MalformedOutput errors instead of Conflict.

## P2

### P2-1 Attempting persisted before pre-effect observation wedges the op; recovery not idempotent when interrupted
- execute:78-83 saves Attempting, then dispatch_prepared observes (release.rs:565,808,865,922). Any pre-effect failure (Conflict, Indeterminate, network, OnDiskVersionDrift, stale remote) leaves Attempting with no effect; after the operator fixes the cause, recover_interrupted requires Exact (execute:175) and Absent -> ReleaseRecoveryUnresolved "do not retry" (state file must be deleted). Also reconstruct runs only if state was missing (execute:61-71): an interrupted or transiently failed reconstruct leaves Pending ops, and pre-existing registry ops then hit dispatch_registry:565 Err (not AlreadySatisfied) one failed run per op. Inconsistent with sibling tag/forge/artifact dispatch, which return AlreadySatisfied on Exact (809-811, 866, 923).
- Workflow uses per-run state under RUNNER_TEMP (callisto-release.yml:381) so mostly masked. Fix: observe before mark_attempting; only persist Attempting immediately before the command; run reconstruct-style adoption on every Pending op in recovery, not just when state missing.

### P2-2 Registry Exact is existence-only; yanked reads Absent; textual "already exists" becomes success unobserved
- release.rs:486-502 return only Exact/Absent (never Conflict) so SRL-07 registry content conflict is unimplemented. `cargo info` yanked -> "could not find" -> Absent (registry_argv.rs:419); publish then returns "already uploaded" -> classify_cargo_output:392-394 (checked before exit status) -> AlreadyPublished -> AlreadySatisfied with no confirmation (release.rs:645-677 only confirms `Published`). npm classifier 434-441 same. Fix: on AlreadyPublished, re-observe and require Exact; map yanked to Conflict.

### P2-3 Artifact manifest `source_commit` is write-only; orchestration revision unbound
- model release.rs:1222 field set at 1256, never read; validate_for_intent (1267-1272) discards `Self::new` result and never compares `self.source_commit` to the intent source; manifest digest (1263) covers the unvalidated field. ReleaseRunProvenanceV1::validate_for_intent (1772-1792) never ties orchestration_revision to slots' workflow_commit; CLI takes `--orchestration-revision` as free input at execute (cli release.rs:389) so a receipt can name a coordinator that did not build/attest. Fix: compare both; require orchestration_revision == policy.workflow_commit when slots exist and artifact_manifest_digest None when none.

### P2-4 Graph-level profile enforcement gaps (SRL-11/SRL-05)
- derive_release_inputs (release.rs:1312-1316) treats an unknown profile as `None`; prepared_registry_binding (1801-1815) silently falls back to the logical (production) route. Rejection lives only in CLI (cli release.rs:205-222, 341-362) and only when `[release]` exists; a `[release]`-less workspace accepts any label against production. Forge repository vs origin is only compared when artifact_policy is Some (1352-1365). Fix: enforce in validate_release_intent/derive.

### P2-5 GitHub release lifecycle semantics
- Published non-draft before assets exist; no `--prerelease` though npm gets `next` (l.1426); assets attach after publication (breaks under GitHub immutable releases -- PLAUSIBLE, repo-setting dependent). Fix: create draft, upload, publish; pass --prerelease for pre-release versions.

### P2-6 E-code misuse
- ReleaseInvariant (E171, "internal defect") used for user config errors: release.rs:1807-1834. Missing/non-exact release reported as ForgeReleaseDiffers (946-948). Bad workflow path reported as UnsafeSlotComponent (model 1081).

## P3
- Hardcoded values: 4 target triples + asset names in release.rs:1254-1262 duplicated in config/resolve.rs TARGETS (config field cannot vary); `.expect("validated product target")` l.1552 depends on that duplicate; timeouts `from_secs(300)` l.759,794; retry=3 and 2^(n+1) backoff l.706-713; `"origin"` l.1934; `github.com` l.2029; npm tag "next" l.1426; `--generate-notes` l.886; attestation flags/timeout release_artifacts.rs:18,171; coordinator workflow path literal in cli release.rs:233; DEFAULT_RATE_LIMIT_WAIT_SECS registry_argv.rs:29.
- model doc says forge-neutral (release.rs:1) but holds GitHub-specific types.
- GitHubRepository compares case-sensitively (release.rs:189-193; graph 1359, cli 216): `Orin-DX/Callisto` remote rejects spuriously (fail-closed).
- `_permit` discarded in dispatch_registry (l.545); stale `#[allow(dead_code)]` (87,125,146,189,207,213) mask test-only `prepared()`.
- is_safe_artifact_component (model 1180) accepts "."; no uniqueness on asset_name; upload path not re-hashed before `gh release upload` (release.rs:936-960); `path#label` gh syntax.
- Empty operation roster yields vacuous success/receipt (model release.rs:1387-1426, execute:189-205).
- CommandFailure::NonZeroExit shows raw unredacted stderr (error.rs:595).
- publish.rs:691 comment refers to a removed "publisher"; only `plan_publish` remains (fine, SRL-14 ok).
- Outside slice, unverified: callisto-release.yml:128 resolves orchestration_sha from `commits/main` at run time; attestation --signer-digest is the dispatch SHA; a moving main would fail closed.

## Positives
- Intent digest is length-prefixed, canonically ordered, binds profile/decision/snapshot/DAG/slots; Deserialize recomputes and rejects mismatch and non-canonical order (model 1339-1374, 1433-1475).
- require_terminal_success + receipt-from-complete-state + fresh Exact observations for every op closes F-007/F-008 in design (execute:189-205; model 2115-2171); Attempting is never downgraded.
- Attempting saved through atomic_write (fsync) before any effect; state validated against intent on every load/save (release_store.rs:83-136).
- Artifact handling is sound: single Normal component, symlink_metadata rejection, canonicalize+starts_with, streaming SHA-256, gh attestation verify pinned to signer workflow/digest (release_artifacts.rs:111-192).
- Distinct typed orchestration vs release-source revisions; 40-hex-only CommitSha; HTTP 404->Absent, other statuses->Indeterminate (release.rs:1222-1228). MIT model has no FSL deps; no unsafe.
