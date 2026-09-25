# Release execution

The types that make a release run safe to crash and rerun, and how the two release routes share them. Behavior: [`docs/specs/release.json`](../specs/release.json). Operator guide: [`docs/releasing.md`](../releasing.md). Decisions: ADRs [2](../adr/0002-committed-decision-is-release-authority.md), [3](../adr/0003-reruns-adopt-landed-effects.md), [4](../adr/0004-ecosystem-tools-publish-and-own-auth.md) and [7](../adr/0007-platform-packages-owned-through-optional-dependencies.md).

## Terms

- Package: a dependency-graph node. It is versioned, cascaded, tagged and published.
- Artifact: the bytes of one *(package, version, target)*, declared by `[[release.artifact]]`. It has no version, tag or changelog of its own. Callisto never parses `target` and never builds; it records where the bytes go.
- Product package: `[release].product-package`. Every artifact slot uploads to the product's one GitHub Release, even when another package built the bytes (`ArtifactSlotId.package`).
- Decision: the package and version roster (`ReleaseDecisionV1`).
- Intent: the decision plus the source snapshot, every operation with its prerequisites, and the artifact slots, sealed by a digest (`ReleaseIntentV1`).
- Operation: one effect on one provider, identified by package, version and role: registry publish, platform publish, tag, forge release (draft), artifact upload, forge publish.

## Layers

| Layer | Crate | Contents |
| --- | --- | --- |
| Authorities | `callisto-model` (MIT) | Run envelope and execution state (`release.rs`), observations and evidence (`release_observation.rs`), transition table (`release_transition.rs`) |
| Derivation and execution | `callisto-graph` | Decisions (`commands/release_decision.rs`), intent derivation (`commands/release/derive.rs`), local route (`commands/release/local.rs`), executor (`commands/release_execution.rs`), provider adapters (`commands/release/provider/`) |
| Entry points | `callisto-cli` | `commands/release.rs`: `release`, `release plan`, `release artifact-manifest`, `release execute` |

## Two routes, one executor

```
local:  derive_unreleased_decision (untagged versions)  -> plan_local_release -> intent
CI:     committed decision file, or --package over plan_version -> release plan -> intent -> build -> release execute
both:   intent -> ReleaseRunEnvelopeV1::new -> execute_release -> ReleaseReceiptV1::from_state
```

- `plan_local_release` is the one derivation behind both `callisto release --dry-run` and the real run.
- `ci_release_route` sends a workspace with artifact slots or platform builds to the CI route before anything runs.
- `expand_selection` (group members) and `require_dependencies_selected` (unselected unreleased dependencies) apply to both routes.

## Run envelope and state (`release.rs`)

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
- GitHub's release-by-tag endpoint omits drafts, so the forge lookup falls back to paging the release list (`provider/forge.rs`).

## Verification tiers

- Fault-injection simulator (`commands/release_simulator.rs`): drives the real `execute_release` against an in-memory provider world, crashing at every provider call and injecting every provider outcome, with each rerun starting fresh. It asserts that a receipt is issued only when every effect really landed and only from state, that no landed effect is re-issued, that no effect lands before its prerequisites, and that each scenario converges within three clean reruns.
- Provider contract tier: fake providers replay captured real responses from `testing/fixtures/providers/`; `TOOL_SHAPES` in `crates/callisto-cli/tests/common/release_harness.rs` rejects flags a fake does not know, and `provider_flag_contract_tests.rs` checks each emitted flag against the real tool. `just provider-fixtures` re-captures and `just provider-contract` checks shapes live; both need the network and are outside `just ci`.
- Wire shapes: decision, intent, envelope and receipt each carry a `SCHEMA_VERSION`; the execution state has no wire shape. `callisto schema --type release-receipt` publishes the receipt schema, guarded by `crates/callisto-cli/tests/schema_guard_test.rs`.
