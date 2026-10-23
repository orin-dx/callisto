# Release execution

The types that make a release run safe to crash and rerun, and how the two release routes share them. Behavior: [`docs/specs/release.json`](../specs/release.json). Operator guide: [`docs/releasing.md`](../releasing.md). Decisions: ADRs [2](../adr/0002-committed-decision-is-release-authority.md), [3](../adr/0003-reruns-adopt-landed-effects.md), [4](../adr/0004-ecosystem-tools-publish-and-own-auth.md) and [7](../adr/0007-platform-packages-owned-through-optional-dependencies.md).

## Terms

- Package: a dependency-graph node owned by a directory; it can expose several canonical manifest/distribution identities.
- Distribution identity: `ReleasePackageId`, an ecosystem-qualified native package name. A graph package with several identities has one tag, one forge release and one forge publish, each carried by the package's owner: its first selected identity in manifest order (Cargo, then npm, then PyPI).
- Artifact: the bytes of one *(package, version, target)*, declared by `[[release.artifact]]`. It has no version, tag or changelog of its own. Callisto does not parse `target`; CI supplies product-slot bytes and Callisto records where they go. Registry publishing can invoke native build/pack tools, including Python's build backend.
- Product package: `[release].product-package`. Every artifact slot uploads to the product's one GitHub Release, even when another package built the bytes (`ArtifactSlotId.package`).
- Decision: the package and version roster (`ReleaseDecisionV1`).
- Intent: the decision plus the source snapshot, every operation with its prerequisites, and the artifact slots, sealed by a digest (`ReleaseIntentV1`).
- Operation: one effect on one provider, identified by package, version and role: registry publish, platform publish, tag, forge release (draft), artifact upload, forge publish. The operations form a DAG by prerequisite; `callisto_model::canonical_operation_order`, built on `michi-graph`, validates it (a cycle is reported as one operation on it) and fixes the order: the smallest topological order by operation id.

## Layers

| Layer | Crate | Contents |
| --- | --- | --- |
| Authorities | `callisto-model` (MIT) | Run envelope and execution state (`release/execution.rs`), observations and evidence (`release_observation.rs`), transition table (`release_transition.rs`) |
| Derivation and execution | `callisto-graph` | Decisions (`commands/release_decision.rs`), intent derivation (`commands/release/derive.rs`), local route (`commands/release/local.rs`), executor (`commands/release_execution.rs`), provider adapters (`commands/release/provider/`) |
| Entry points | `callisto-cli` | `commands/release.rs`: `release`, `release plan`, `release build-python`, `release artifact-manifest`, `release execute` |

## Two routes, one executor

```
local:  derive_release_roster (untagged versions, plus tagged-on-HEAD releases not fully observed)  -> plan_local_release -> intent
CI:     committed decision file, or --package over plan_version -> release plan -> intent -> build -> release execute
both:   intent -> ReleaseRunEnvelopeV1::new -> execute_release -> ReleaseReceiptV1::from_state
```

- `plan_local_release` is the one derivation behind both `callisto release --dry-run` and the real run.
- A tag proves only that the tag was created. `incomplete_tagged_releases` observes every operation of the packages tagged on HEAD and keeps any not exactly satisfied; a tag on another commit is never observed.
- `ci_release_route` sends a workspace with artifact slots or platform builds to the CI route before anything runs.
- `expand_selection` (group members) and `require_dependencies_selected` (unselected unreleased dependencies) apply to both routes.

## Managed release PR and workflow

Callisto's own [release workflow](../../.github/workflows/callisto-release.yml) reconstructs one `callisto/version-packages` PR from `main` when changesets land. It versions manifests and changelogs; merging the PR consumes the changesets on `main` and authorizes publication.

- The action commits non-workflow changes through `createCommitOnBranch`, then moves the release ref through REST. It does not run a local `git push`. Workflow files remain unchanged, so the built-in `GITHUB_TOKEN` needs no workflow-write permission.
- Commits are GitHub-signed as `github-actions[bot]`. An App or fine-grained token can use another identity.
- Branch protection must require reviews and CI checks. CODEOWNERS alone does not enforce review.
- `version-pr` maintains the PR; `release` runs the local release route. Artifact and platform packages use plan/build/execute. Deprecated `publish` and `create_github_release` action inputs are ignored. The [action manifest](../../.github/actions/callisto-action/action.yml) defines current inputs and outputs.
- Only `execute` receives registry credentials. Plan and build jobs receive none. Temporary intent, receipt, artifacts, and build output live under `RUNNER_TEMP`, outside the checkout.

GitHub Actions schedules jobs, transports artifacts, and injects credentials. Callisto validates source, provenance, plan, observations, transitions, terminality, receipts, and artifacts. Workflow shell must not reimplement those semantics. Recovery dispatch uses current orchestration on `main` with an explicit merged source SHA and runs credential-free `recovery-checks` instead of full `verify`.

The generated release action maps credentials without changing owner-defined publish hooks:

- npm: copy existing user config to a mode-0600 temporary file, add an npmjs entry interpolating `NPM_TOKEN`, point npm-compatible tools at it, and remove it on exit. The original config is unchanged. `NPM_TOKEN` remains available for native hooks and `.npmrc` interpolation. Custom registries use native configuration.
- Bun: also set `NPM_CONFIG_TOKEN`, because Bun does not honor `NPM_CONFIG_USERCONFIG`.
- PyPI: map `PYPI_TOKEN` to `TWINE_USERNAME=__token__` and `TWINE_PASSWORD` for upload. The build backend receives no registry or forge credentials.
- Cargo: pass `CARGO_REGISTRY_TOKEN` through to Cargo.

## Run envelope and state (`release/execution.rs`)

- `ReleaseRunEnvelopeV1` is one run's identity: orchestration revision, release-source revision, intent digest, artifact-manifest digest.
- `ReleaseRunEnvelopeV1::new` is its only constructor. The source revision and intent digest are read from the intent, so a caller cannot assert them; the constructor cross-checks the rest against the intent before any effect.
- `ReleaseExecutionStateV1::new(intent, envelope)` is the only way to start a state, so no state exists without a validated envelope.
- The state is never persisted or loaded. Every run starts with every operation `Pending`.
- `ReleaseReceiptV1::from_state` builds the receipt from the state alone: the envelope plus the evidence each operation recorded on success. Nothing re-observes providers after the effects.

## Observation and evidence (`release_observation.rs`)

- `ProviderObservationV1` is `Absent`, `Exact { evidence }`, `Conflict { reason }` or `Indeterminate { cause }`. Reasons and causes are closed enums.
- `ProviderEvidenceV1` is closed and per role: registry version, git tag peeled commit, forge release tag and draft flag, artifact size and sha256.
- `ReleaseOperationObservationV1::new` rejects evidence whose shape does not belong to the operation's role; deserialization goes through it too.
- Proof tokens: `AbsentProof` is minted only from `Absent`, `ExactEvidence` only from `Exact`.
- `execute_release` observes each operation once before its effect: exact is adopted, absent is performed then confirmed, anything else stops the run before the effect.
- An npm registry publish is the exception to "then confirmed": it is issued, stays `Attempting`, and another npm publish may follow it. When nothing else can start, the executor looks at all issued publishes together until each is shown or the longest `confirm-timeout` is spent. That window is one `ConfirmationWave`: it opens when the first publish is issued, is measured on the provider set's clock so looking counts, and a publish shown along the way does not restart it. The tag, release draft, uploads and publication wait for the confirmation, so nothing is announced before the registry shows the version. Cargo and PyPI publishes are confirmed as they are published (PyPI completion is a file set, not a version).

Provider reads use native tools for Cargo and npm, and `curl` for PyPI's PEP 691 JSON simple index:

- Cargo: `cargo info NAME@VERSION --registry REGISTRY` runs from the source root. Matching `version:` means exact; exit 101 with ``could not find `NAME@VERSION` `` means absent. Other results are indeterminate. Yanked versions are invisible to `cargo info`; a rejected publish ends without a receipt.
- npm: `npm view NAME@VERSION version --json`.
- PyPI: a matching file means exact; 404 or no match means absent; transport failure or non-JSON means indeterminate. Yanked matches count as absent. Before an effect, `release execute` also builds the release's files once into a temporary directory and compares their names with the index, so a partly uploaded release uploads only what is missing and is satisfied only when the whole set is there.

Adopting a version that is already published is by design, so a rerun can finish a half-done release. Because a foreign package would be adopted the same way, a Cargo or npm adoption also reads the repository the registry records for that version (`repository:` in `cargo info`, `repository` in `npm view`) and warns, without blocking, when it names a different repository from `origin`. A registry that records none, or a failed read, says nothing; PyPI's simple index records no repository, so it is not checked.

These reads are version-level checks, not proof of identical bytes or the npm dist-tag. Registry endpoints must be HTTPS. Read-only observations before an effect retry 429, 5xx, rate-limited 403, and transport failures with backoff and `Retry-After`. Effects are never blindly retried; outbound commands have deadlines.

After a registry publish the version may not be readable yet: npm serves a package's document with `max-age=300`, a missing package's 404 included, so a read just before the publish can answer stale for five minutes. The confirmation therefore waits by time, not tries, treating absent and unreachable as lag until the registry's window (`confirm-timeout`, default 6 minutes for npm and PyPI, 2 for Cargo) is spent, then fails with E157. An exact version or a conflict ends the wait at once.

## Transition table (`release_transition.rs`)

`transition` is the only function that computes an `OperationState`; `ReleaseExecutionStateV1::apply` is the only mutator. The whole legal edge set:

| From | Event | To |
| --- | --- | --- |
| Pending | `Attempt { AbsentProof }` | Attempting |
| Pending | `ObservedExactBeforeEffect { ExactEvidence }` | AlreadySatisfied |
| Attempting | `Confirmed { ExactEvidence }` | Published |
| Attempting | `EffectFailedAndAbsent { AbsentProof }` | Failed |
| Pending or Attempting | `Blocked { reason }` | Blocked |

- Any other pair is an `InvalidTransition`.
- `Attempting` is unreachable without a proof of absence.
- `Published` (our effect landed) stays distinct from `AlreadySatisfied` (it existed before this run).

## Forge ordering

- A GitHub release is created as a draft, assets upload to the draft, and publication is a separate last operation that depends on every upload. A published release can be immutable and refuse new assets.
- GitHub's release-by-tag endpoint omits drafts, so the forge lookup finds the release ID by tag through GraphQL, then reads the release by ID (`release/github.rs`).
- GitHub's reads lag its writes by seconds. The check right after this run's own create, upload or publish retries an absent answer for about a minute before it reports a conflict (`observe_until_visible`).

The tag/announcement barrier covers all required publications of a graph package: a package's tag waits for the registry publishes of every release identity it has (`OpIndex::tag_prerequisites`), so a dual Cargo/npm owner's one tag and public release follow both publishes, in no fixed ecosystem order. Selecting one identity of a dual package releases every identity of that package, so the tag never waits on a subset of its publishes. A product's public release also waits for the tag, and so the registry publishes, of each other package whose asset it carries. An intent derived before either rule fails closed as stale and is rebuilt with `release plan`. See [the scoped design](polyglot-v1.md) for proposed ownership and future Go tag-publication semantics.

## Wire and observation boundaries

Stdout JSON reports include `schemaVersion`, `command` and `dryRun`; durable decision, intent, artifact-manifest and receipt files retain their own wire shapes. The CLI's `release-pr verify` reader accepts both the strict bare decision and unchanged decide report, validating the report metadata before passing the payload to the shared domain constructor. Unknown and duplicate fields remain errors; durable wire readers are unchanged. The real-binary action contract exercises this producer/consumer boundary through inline, file and stdin inputs.

Managed PR verification compares the original snapshot before staging and before moving the release ref. After a successful update, the action confirms the ref's exact new SHA and supplies it as `--applied-head` for the final check. The model validates the original update decision and allows only its selected PR's head to change to that SHA; the base, repository, PR roster and every other head remain unchanged. Create decisions keep strict equality until the PR is created. The action does not rewrite its decision or discard concurrency checks after its own effect.

Registry evidence currently confirms a version, with optional checksum/yank fields; it does not establish byte identity. Python completion has two modes. Without a sealed file set (the local route, and a CI route whose manifest seals none) it is judged against the files the executing run builds, matched by name, and digests are compared only for files that run uploaded: a rebuild need not be byte-identical to an earlier attempt. With a sealed set, a build job (`release build-python`, no credentials) builds the files and `release artifact-manifest --seal-python` records each file's name, size and sha256 in the manifest, so the envelope's manifest digest covers them. `release execute` verifies the bytes, builds nothing, uploads those files, and treats any index file under a sealed name with another sha256 as an error (E307). The sealed sets are all or none per plan and each belongs to a Python publish of the intent; the manifest stays at schema version 1 because a manifest without sets is unchanged and an older reader rejects the new field. Artifact size/sha256 validation otherwise applies to declared GitHub assets. A PyPI registry reads its simple index at `url` and uploads to `publish-url` when set, else `url`; both endpoints are part of the registry binding. Authenticated reads and HTML-only indexes are not supported.

## Verification tiers

- Fault-injection simulator (`commands/release_simulator.rs`): drives the real `execute_release` against an in-memory provider world, crashing at every provider call and injecting every provider outcome, with each rerun starting fresh. It asserts that a receipt is issued only when every effect really landed and only from state, that no landed effect is re-issued, that no effect lands before its prerequisites, and that each scenario converges within three clean reruns.
- Provider contract tier: fake providers replay captured real responses from `testing/fixtures/providers/`; `TOOL_SHAPES` in `crates/callisto-cli/tests/common/release_harness.rs` rejects flags a fake does not know, and `provider_flag_contract_tests.rs` checks each emitted flag against the real tool. `just provider-fixtures` re-captures and `just provider-contract` checks shapes live; both need the network and are outside `just ci`.
- Wire shapes: decision, intent, envelope and receipt each carry a `SCHEMA_VERSION`; the execution state has no wire shape. `callisto schema --type release-receipt` publishes the receipt schema, guarded by `crates/callisto-cli/tests/schema_guard_test.rs`.

## Installer and product assets

`setup-callisto` defaults to `source: auto`: install the requested release, even in a Callisto checkout. An explicit `version` wins; otherwise `latest` resolves the pinned action ref to its `callisto@` release when possible. A `callisto-cli` manifest alone does not select checkout source.

`source: local` builds the checkout and ignores `version`. It requires tracked root and CLI manifests, the CLI entrypoint, a Cargo-confirmed workspace member and binary target, Git, Cargo, jq, and locked offline metadata resolution. It uses the checkout's `target/` cache, installs under `RUNNER_TEMP`, and never falls back from invalid local source. Callisto's release jobs use this mode.

Downloaded binaries are checked with `gh attestation verify` against `orin-dx/callisto` and the `callisto-release.yml` signer before extraction. Only the `callisto` binary is extracted. `verification: require` aborts on failure; `fallback` (default) deletes the asset and installs from crates.io; `skip` warns and uses it. Fallback pins the requested version except for `latest`. Older unattested releases motivate the default. `require` and `fallback` never run an unverified asset.

Product assets target `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, and `x86_64-unknown-linux-musl`. The Intel macOS asset is cross-compiled on an Apple silicon runner, and the arm64 Linux asset is built natively on an arm64 runner. Apple signing and notarization are out of scope. GitHub provenance binds assets to the orchestration revision; the intent and artifact manifest bind their bytes to the release source. Callisto publishes its crates to crates.io and product assets to GitHub Releases, not to another registry for rehearsal.
