# Release Lifecycle — Envelope, Evidence, Transitions

The three structural authorities of a durable release run. All three live in `callisto-model` (MIT, no Callisto dependencies); `callisto-graph` supplies provider adapters, `callisto-cli` supplies the run entry point.

## Two routes, one executor

- **Local** (`callisto release`, `commands/release/local.rs`): `derive_unreleased_decision` selects every publishable package whose current version has no tag in the `TagIndex` (`ReleaseInclusionReason::UnreleasedVersion`); `--package` restricts to named unreleased packages, expanded by `expand_selection` (shared with the CI route: fixed and linked group membership from config, plus plan-derived linked reasons). `require_dependencies_selected` (both routes) refuses an unselected, unreleased, publishable runtime/optional/peer dependency. `plan_local_release` is the one derivation behind both `--dry-run` (`Preview`: HEAD's commit, any worktree state; no `origin` leaves tags unbound with a stderr note) and the run (`Trusted`: clean worktree, any branch, `ReleaseCheckout::AnyHead`). The run stops first on `ci_release_route` (artifact slots, a platform-role manifest, attached platforms, or a napi/maturin matrix target), then checks one credential per operation (CLI `release_credentials.rs`: per cargo registry, per npm package dir, per PyPI repository, GitHub), then mints the envelope with orchestration revision = source commit, validates with `validate_local_release_intent`, and calls `execute_release`. Receipt only on full success: `--receipt`, else `<state dir>/callisto/<sha256(canonical root)[..16]>/<intent digest>/receipt.json`; either is refused inside the worktree. No-op prints exactly `Nothing to release.`.
- **CI** (`release plan`/`artifact-manifest`/`execute`): exact detached merge commit (`ReleaseCheckout::Detached`), decision from `--from-release-commit` + committed decision file, or `--package` over the version plan.

The legacy `publish`, `plan-publish`, `tag`, and `filter-plan` commands and graph `plan_publish`/`create_tags` are gone.

## Run envelope (`release.rs`)

`ReleaseRunEnvelopeV1` is the immutable identity of one run: orchestration revision, release-source revision, intent digest, artifact-manifest digest. There is one run kind and one destination (`[release].forge-repository`).

- One constructor, `ReleaseRunEnvelopeV1::new(orchestration_revision, intent, manifest_digest)`. Source revision and intent digest are read out of the intent, so they have no second authority and cannot be asserted by a caller.
- Cross-field rules, enforced before the first effect: source revision equals the intent's Git source; manifest digest is `Some` exactly when the intent declares artifact slots; the orchestration revision equals every slot's attestation `workflow_commit`.
- It is held **inside** `ReleaseExecutionStateV1`, whose only constructor is `new(intent, envelope)`. The state is in memory only: it is never persisted or loaded, and every run (including a CI "Re-run failed jobs") starts from all-`Pending`. The receipt is built from the state alone (`ReleaseReceiptV1::from_state`): its envelope plus the `ProviderEvidenceV1` each operation recorded on reaching `Published`/`AlreadySatisfied`. Nothing re-observes providers after the effects.
- `execute_release` observes each operation once before any effect: `Exact` becomes `AlreadySatisfied` for every role (a registry version it adopts prints one `warning: <package> <version> is already published; skipping` line), `Absent` is published and confirmed, `Conflict` is E167 and `Indeterminate` is E176.

## Observation and evidence (`release_observation.rs`)

`ProviderObservationV1` is `Absent | Exact { evidence } | Conflict { reason } | Indeterminate { cause }`. `ProviderEvidenceV1` is closed and per-role: `RegistryVersion { version, checksum?, yanked? }`, `GitTag { peeled_commit }`, `ForgeRelease { tag_name, draft }`, `ArtifactUpload { byte_length, sha256 }`. `Option` fields are `None` where the adapter cannot yet prove the value. The `ForgeRelease` evidence serves both forge roles; for `ForgePublish` it is only accepted with `draft: false`.

`ReleaseOperationObservationV1::new` is the single constructor, used by deserialization too, and rejects evidence whose shape does not match the operation's role. Conflict reasons and indeterminate causes are closed enums; an authentication, rate-limit, or transport failure is always indeterminate, never a conflict.

Two tokens are mintable only from an observation: `AbsentProof` (from `Absent`) and `ExactEvidence` (from `Exact`).

## Registry observation through the package manager (`provider/registry.rs`)

Observation runs the ecosystem's own package manager -- the same tool and the same configuration the publish effect uses -- through the bounded `CommandRunner`, so registry resolution and authentication (private registries included) belong to the tool rather than to a credential-free reimplementation.

- cargo: `cargo info NAME@VERSION --registry REGISTRY`, run from the source workspace root so that workspace's `.cargo/config.toml` registry definitions and credentials apply. `--registry` is never omitted: without it `cargo info` resolves the local workspace and reports an unpublished member's on-disk version as published, which is the D01 defect. The logical `cratesIo` key maps to cargo's built-in `crates-io` (`cargo_registry_name`).
- npm: `npm view NAME@VERSION version --json`, reporting `checksum: None` rather than inventing one.
- npm platform packages (`os`/`cpu` set, listed in an owner's `optionalDependencies`) get a `PlatformPublish { registry, platform }` operation, published by directory before their owner.
- PyPI: `curl -sS -i` against the PEP 691 JSON simple index (not pip, which cannot tell a missing project from an unreachable index). 200 + matching file is `Exact`; a yanked file, no match, or 404 is `Absent` (fail closed, like cargo); transport failure or a non-JSON (PEP 503 HTML) body is `Indeterminate`. Parsed with the same `http::parse_http_response` as `gh api --include`.

Cargo classification (`classify_cargo_info`): exit 0 with a `version: VERSION` line for the requested version is `Exact` with `checksum: None, yanked: None` (`cargo info` reports neither, and the evidence does not invent them); exit 101 whose stderr carries ``could not find `NAME@VERSION` `` is `Absent`; anything else -- another exit code, another message, or a zero exit with no matching version line -- is a transient `Indeterminate` under the retry policy, so an unreachable registry can never read as an absence.

A **yanked** version reads as `Absent`, because `cargo info` answers "could not find" for one. This fails closed: the publish that follows is refused by the registry, and since only an `Exact` observation may satisfy an operation, the run ends in `RegistryPublishUnconfirmed`, never in a receipt. `ProviderConflictReason::RegistryVersionYanked` remains in the wire enum but no adapter currently produces it.

Registry endpoints must be `https` (`registry_endpoint::canonical_registry_url`), with no loopback exception, enforced by release binding (E126). A publish target binds its own registry key directly; there is no per-profile route.

## Forge roles and DAG order (`provider/forge.rs`, `provider/artifact.rs`)

Publication is the last forge step, because GitHub releases can be immutable: once published, assets can no longer be added, so a release must be complete before it is published.

`Tag` -> `ForgeRelease` (create as draft) -> every `ArtifactUpload` -> `ForgePublish`. `ForgePublish` depends on the draft and on *every* upload of that release, including when there are none.

- `ForgeRelease` effect: `gh release create TAG --repo R --verify-tag --draft`, with `--notes-file` holding the package's `## VERSION` changelog section, or `--generate-notes` plus a `notes: using generated notes (<reason>) for <package>` stderr line when none is usable (`ReleaseNotes`, derived at execution and outside the intent digest), plus `--prerelease` when the released version has a semver pre-release part (`Version::is_prerelease()`, the same source as the npm `next` dist-tag).
- `ForgePublish` effect: `gh release edit TAG --repo R --draft=false`.
- Lookup: `GET /releases/tags/{tag}` omits drafts, so a 404 there is followed by a bounded scan of `GET /releases?per_page=100&page=N` (at most 10 pages). Both roles and the upload role share this one lookup.
- `ForgeRelease` is `Exact` for a draft or a published release; `ForgePublish` is `Exact` only when `draft: false` and `Absent` while still a draft. A release whose tag matches but whose `prerelease` flag disagrees with the version is `Conflict { ForgeReleasePrereleaseDiffers }`.
- Uploads target the draft, compare by size and `sha256` digest, and never pass `--clobber`. An asset missing from a published release is `Absent`, so it is uploaded; an immutable release refuses that upload as a typed command failure rather than letting the release be reported complete.

## Artifact ownership (`config/resolve.rs`, `commands/release/derive.rs`)

A package is a dependency-graph node: versioned, cascaded, tagged, published. An artifact is a rendering of *(package, version, target)* into bytes. It has no tag, changelog or version of its own; it inherits them from its package.

- `[[release.artifact]]` declares `package`, an opaque `target` Callisto never parses, and a stable `asset-name`. Nothing about a product is compiled in.
- `ArtifactSlotId.package` is the package whose build produced the bytes, which need not be the product. Every slot still attaches to the product's one GitHub Release (the product's `ForgeRelease`/`ForgePublish`).
- `ArtifactSlotOutsideDecision` requires the owning package in the decision at the slot's version. `--package` narrowing therefore keeps every member of a selected package's `[[fixed-group]]` (membership from config, since the selected member's own reason is usually `Changeset`).
- Building is delegated. Callisto models the outputs and where they go, not how each ecosystem cross-compiles. napi platform packages and Python wheels go to their registries (publish path), not to `[release]`.

## Bounded retry (`provider/policy.rs`)

`retry_observation` wraps read-only observations only -- registry HTTP, the GitHub release GET, `git ls-remote`. An effect is never re-issued. Retryable: HTTP 429, any 5xx, a 403 carrying rate-limit headers, and a curl timeout or connection failure. Five attempts, 2s doubling to 16s, each wait capped at 300s, honouring `Retry-After` (delta-seconds or HTTP-date). Post-publish confirmation uses the same policy with `Absent` treated as index-propagation lag. `Sleeper` is the injected wall clock.

## Transition table (`release_transition.rs`)

`transition(current, event)` is the only place an `OperationState` is computed; `ReleaseExecutionStateV1::apply` is the only mutator. The whole legal edge set:

| From | Event | To |
|---|---|---|
| Pending | `Attempt { AbsentProof }` | Attempting |
| Pending | `ObservedExactBeforeEffect { ExactEvidence }` | AlreadySatisfied |
| Attempting | `Confirmed { ExactEvidence }` | Published |
| Attempting | `EffectFailedAndAbsent { AbsentProof }` | Failed |
| Pending/Attempting | `Blocked { reason }` | Blocked |

Consequences: `Attempting` is unreachable without a proven-absent provider; "our attempt landed" (`Published`) is distinct from "it already existed" (`AlreadySatisfied`); terminal states absorb every event.

## Fault-injection simulator (`commands/release_simulator.rs`)

A deterministic in-crate simulator drives the real `execute_release` plus `ReleaseReceiptV1::from_state` against an in-memory provider set, enumerating every crash point (each provider call) and every provider fault (indeterminate, conflict, effect-fails-before-landing, effect-lands-then-error, registry lag), plus every crash-and-fault pair, with each rerun starting fresh.

Asserted after every scenario: a receipt only over landed effects, issued with no provider call and recording exactly the evidence the world holds; no effect re-issued for an operation already landed; no effect landing before every prerequisite; nothing downstream of an observed conflict landing; and convergence within three clean reruns -- except a rerun re-issuing an effect a lagging index still reports absent, which only the provider can refuse.

## E-codes

- `E172` release execution incomplete (non-terminal operations remain).
- `E176` provider indeterminate before dispatch.
- `E177` provider evidence does not belong to the operation's role, raised when the state would record it (internal defect).
- `E178` run envelope invalid for this intent.
- `E179` an artifact's `package` is not part of this release (user config, not a defect).
- `E180` tag push refused by GitHub's App workflow guard (`GITHUB_TOKEN` pushing a commit whose `.github/workflows/` differs from every branch tip, i.e. releasing an older source). Detected from the push stderr in `provider/tag.rs::push_tag`; any other push failure stays `E164`. Remedy: push the tag with a non-App credential, then re-run the release.

## Wire versions

`ReleaseExecutionStateV1` has no wire shape. `ReleaseDecisionV1::SCHEMA_VERSION` is `2` (adds `unreleasedVersion`); `READABLE_SCHEMA_VERSIONS` is `[1, 2]` so a committed v1 decision still plans; `ReleaseReceiptV1::SCHEMA_VERSION` is `2`; its `ReleaseRunEnvelopeV1::SCHEMA_VERSION` is `3` (run `kind`, then `profile`, removed); `ReleaseIntentV1::SCHEMA_VERSION` is `5` (`profile` removed). An intent from an earlier version is rejected by `callisto::release_intent_schema_unsupported`, which names re-planning as the fix. `callisto schema --type release-receipt` publishes the wire shape, guarded by `crates/callisto-cli/tests/schema_guard_test.rs`.

## Provider contract tier

Fake providers serve captured real responses and captured real command runs (`testing/fixtures/providers/<provider>/`, provenance in each `PROVENANCE.md`; `loopback::fixtures` in `testing/loopback_http.rs` reads the HTTP captures and the `.cmd` envelopes and templates names and versions into them), every fake rejects flags and subcommands outside `TOOL_SHAPES` in `tests/common/release_harness.rs`, and `provider_flag_contract_tests.rs` checks each emitted flag against the real tool's own help. The cargo observation is additionally tested against real `cargo info` over a hermetic local git registry served by `file://` (`provider/registry/tests.rs`). Refresh fixtures with `just provider-fixtures` (read-only GETs; review the diff, update `PROVENANCE.md`); `just provider-contract` re-fetches live and asserts shapes still match. Both are manual and network-dependent, not part of `just ci`.
