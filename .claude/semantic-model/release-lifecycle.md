# Release Lifecycle — Envelope, Evidence, Transitions

The three structural authorities of a durable release run. All three live in
`callisto-model` (MIT, no Callisto dependencies); `callisto-graph` supplies
provider adapters, `callisto-cli` supplies the run entry point.

## Run envelope (`release.rs`)

`ReleaseRunEnvelopeV1` is the immutable identity of one run: kind
(`Initial`/`Recovery`), orchestration revision, release-source revision,
profile, intent digest, artifact-manifest digest.

- One constructor, `ReleaseRunEnvelopeV1::new(kind, orchestration_revision, intent, manifest_digest)`.
  Profile, source revision, and intent digest are read out of the intent, so
  they have no second authority and cannot be asserted by a caller.
- Cross-field rules, enforced before the first effect: source revision equals
  the intent's Git source; manifest digest is `Some` exactly when the intent
  declares artifact slots; the orchestration revision equals every slot's
  attestation `workflow_commit`.
- It is persisted **inside** `ReleaseExecutionStateV1`, whose only constructor
  is `new(intent, envelope)`. The receipt is built from the state's envelope
  plus fresh observations — nothing is assembled after the effects.
- `validate_for_run` rejects state left by a different run
  (`ReleaseStateError::MismatchedEnvelope`, message names `--state`).

## Observation and evidence (`release_observation.rs`)

`ProviderObservationV1` is `Absent | Exact { evidence } | Conflict { reason } |
Indeterminate { cause }`. `ProviderEvidenceV1` is closed and per-role:
`RegistryVersion { version, checksum?, yanked? }`, `GitTag { peeled_commit }`,
`ForgeRelease { tag_name, draft }`, `ArtifactUpload { byte_length, sha256 }`.
`Option` fields are `None` where the adapter cannot yet prove the value.
The `ForgeRelease` evidence serves both forge roles; for `ForgePublish` it is
only accepted with `draft: false`.

`ReleaseOperationObservationV1::new` is the single constructor, used by
deserialization too, and rejects evidence whose shape does not match the
operation's role. Conflict reasons and indeterminate causes are closed enums;
an authentication, rate-limit, or transport failure is always indeterminate,
never a conflict.

Two tokens are mintable only from an observation: `AbsentProof` (from `Absent`)
and `ExactEvidence` (from `Exact`).

## Registry observation over the protocol (`provider/http.rs`, `provider/registry.rs`)

Observation never shells to a package-manager client that can resolve the local
workspace. It is a credential-free `curl` GET through the bounded
`CommandRunner`, against the profile-bound endpoint:

- cargo: the sparse index, `{endpoint}/{1|2|3/a|ab/cd}/{name}` lowercased. A
  cargo registry configured without the `sparse+` marker is a git index and
  fails closed as `Indeterminate { UnsupportedProtocol }`. The built-in
  `cratesIo` key is always sparse, and may carry a `url` to point it at another
  http host (rehearsal, tests) under the usual validation and E197 rules.
- PyPI: `{endpoint}/pypi/{pep503-name}/{version}/json`. PyPI is now observable,
  so a PyPI publish can reach a receipt.
- npm: still `npm view`, reporting `checksum: None` rather than inventing one.

404 or a missing version line is `Absent`; a served, unyanked version is `Exact`
with the index checksum and `yanked: Some(false)`; a served, yanked version is
`Conflict { RegistryVersionYanked }` -- the defect `cargo info` hid by reading a
yanked version as absent. Any other status, or a body that is not the index
format, is indeterminate.

## Forge roles and DAG order (`provider/forge.rs`, `provider/artifact.rs`)

Publication is the last forge step, because GitHub releases can be immutable:
once published, assets can no longer be added, so a release must be complete
before it is published.

`Tag` -> `ForgeRelease` (create as draft) -> every `ArtifactUpload` -> `ForgePublish`.
`ForgePublish` depends on the draft and on *every* upload of that release,
including when there are none.

- `ForgeRelease` effect: `gh release create TAG --repo R --verify-tag --draft
  --generate-notes`, plus `--prerelease` when the released version has a semver
  pre-release part (`Version::is_prerelease()`, the same source as the npm
  `next` dist-tag).
- `ForgePublish` effect: `gh release edit TAG --repo R --draft=false`.
- Lookup: `GET /releases/tags/{tag}` omits drafts, so a 404 there is followed by
  a bounded scan of `GET /releases?per_page=100&page=N` (at most 10 pages).
  Both roles and the upload role share this one lookup.
- `ForgeRelease` is `Exact` for a draft or a published release; `ForgePublish`
  is `Exact` only when `draft: false` and `Absent` while still a draft. A
  release whose tag matches but whose `prerelease` flag disagrees with the
  version is `Conflict { ForgeReleasePrereleaseDiffers }`.
- Uploads target the draft, compare by size and `sha256` digest, and never pass
  `--clobber`. An asset missing from a published release is `Absent`, so it is
  uploaded; an immutable release refuses that upload as a typed command failure
  rather than letting the release be reported complete.

## Bounded retry (`provider/policy.rs`)

`retry_observation` wraps read-only observations only -- registry HTTP, the
GitHub release GET, `git ls-remote`. An effect is never re-issued. Retryable:
HTTP 429, any 5xx, a 403 carrying rate-limit headers, and a curl timeout or
connection failure. Five attempts, 2s doubling to 16s, each wait capped at
300s, honouring `Retry-After` (delta-seconds or HTTP-date). Post-publish
confirmation uses the same policy with `Absent` treated as index-propagation
lag. `Sleeper` is the injected wall clock.

## Transition table (`release_transition.rs`)

`transition(current, event, run_kind)` is the only place an `OperationState`
is computed; `ReleaseExecutionStateV1::apply` is the only mutator and takes the
run kind from its own envelope. The whole legal edge set:

| From | Event | Kind | To |
|---|---|---|---|
| Pending | `Attempt { AbsentProof }` | any | Attempting |
| Pending | `ObservedExactBeforeEffect { ExactEvidence }` | any | AlreadySatisfied |
| Pending | `AdoptedExact { ExactEvidence }` | Recovery | AlreadySatisfied |
| Attempting | `Confirmed { ExactEvidence }` | any | Published |
| Attempting | `RecoveredExact { ExactEvidence }` | any | Published |
| Attempting | `EffectFailedAndAbsent { AbsentProof }` | any | Failed |
| Pending/Attempting | `Blocked { reason }` | any | Blocked |

Consequences: `Attempting` is unreachable without a proven-absent provider;
adopting a pre-existing effect into a missing journal is a recovery-only
privilege; "our attempt landed" (`Published`) is distinct from "it already
existed" (`AlreadySatisfied`); terminal states absorb every event.

## E-codes

- `E172` release execution incomplete (non-terminal operations remain).
- `E173` recovery unresolved: an `Attempting` or adopted operation is absent,
  conflicting, or indeterminate. Do not retry the effect.
- `E174` registry version already exists at preflight.
- `E176` provider indeterminate before dispatch.
- `E177` provider observation unusable as evidence (role mismatch; internal defect).
- `E178` run envelope invalid for this intent.

## Wire versions

`ReleaseExecutionStateV1::SCHEMA_VERSION` and `ReleaseReceiptV1::SCHEMA_VERSION`
are both `2`; `ReleaseIntentV1::SCHEMA_VERSION` is `3` (the `forgePublish`
role). An intent from an earlier version is rejected by
`callisto::release_intent_schema_unsupported`, which names re-planning as the
fix. `callisto schema --type release-receipt|release-state` publishes
the wire shape, guarded by `crates/callisto-cli/tests/schema_guard_test.rs`.

## Provider contract tier

Fake providers serve captured real responses (`testing/fixtures/providers/<provider>/`, provenance in each `PROVENANCE.md`; `LoopbackResponse::from_raw_http` and `loopback::fixtures` in `testing/loopback_http.rs` template names and versions into them), every fake rejects flags and subcommands outside `TOOL_SHAPES` in `tests/common/release_harness.rs`, and `provider_flag_contract_tests.rs` checks each emitted flag against the real tool's own help.
Refresh fixtures with `just provider-fixtures` (read-only GETs; review the diff, update `PROVENANCE.md`); `just provider-contract` re-fetches live and asserts shapes still match. Both are manual and network-dependent, not part of `just ci`.
