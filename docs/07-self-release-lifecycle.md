# Self-release lifecycle

This document is the operational contract for Callisto's own releases. The implementation plan
is [`SPEC-SELF-RELEASE-LIFECYCLE`](projects/SPEC-SELF-RELEASE-LIFECYCLE.json).

## Authority

A verified merge of a managed Callisto release PR is the sole authorization for publication.
There is no GitHub Environment reviewer, deployment protection gate, or second manual click.
Merge evidence and release authorization are separate concerns: required CI checks protect the
merge; the merge authorizes the release.

## Merge evidence

The active `main` ruleset requires one approving review, signed commits, linear squash history,
and the following named evidence before a release PR can merge:

- `Workflow Contracts`
- `Code Formatting (Moon & Just)`
- `Clippy Lints (Moon & Just)`
- `Security & Advisory Audit (Moon & Just)`
- `Code Coverage & Test Report`
- `Test Suite & WASM Check (macos-latest)`
- `Test Suite & WASM Check (ubuntu-latest)`
- `Release Artifact Preflight (macos-arm64)`
- `Release Artifact Preflight (linux-gnu)`
- `Release Artifact Preflight (linux-musl)`
- `Release Artifact Preflight (wasm-wasi)`
- `Validate Changesets & Workspace Status`

The repository owner has an explicit ruleset bypass for emergency recovery. That bypass is not a
second approval mechanism and must be used only when the normal evidence path cannot run.

## Release identity and recovery

Every release run records two different revisions:

- The orchestration revision: the workflow and Callisto CLI that coordinate the run.
- The release-source revision: the merged release commit whose exact versions are released.

A recovery is a new run using current orchestration and an explicit full release-source SHA. It
does not rerun historical workflow code, create a version PR, or invent a newer version.

The orchestration revision is the SHA of the run itself (`github.sha`, main only), never the moving
tip of `main`, because attestations are stamped with the run's SHA. A recovery dispatch is gated by
`recovery-checks` (release workflow contract and policy checks), not the full `verify` job, so an
unrelated failure on `main` cannot block publishing an already merged release.

The immutable run envelope and provider observations are recovery authority. A local state file
is useful crash evidence, but cannot prove what a registry, Git remote, or forge contains.

The selected release profile is part of the immutable intent digest, not just a receipt label.
The production profile declares its GitHub forge destination in `callisto.toml`; planning rejects
an artifact repository that differs from that destination, and execution rejects a profile that
does not match its intent. An unconfigured profile fails before it writes an intent or dispatches
an effect.

Each configured profile also declares `registry-routes`, mapping a logical package target such as
`cratesIo` to a concrete configured registry key. The resolved key and its credential-free
endpoint digest are part of the release operation and package fingerprint. A configured profile
cannot fall back to an un-routed production registry, and two profiles cannot share a forge or
configured registry destination.

## Execution

Before an effect, Callisto observes the provider. An operation is one of absent, exact success,
conflict, or indeterminate. Exact success becomes `AlreadySatisfied`; Callisto must not publish
the same version merely to learn whether it exists. Conflict and indeterminate observations fail
closed with an actionable diagnostic.

The release succeeds only when every operation is provider-observed as exact success or already
satisfied. `Attempting`, failed, blocked, missing, and indeterminate are non-success states. A
receipt is emitted only after that global condition holds.

## Workflow boundary

GitHub Actions schedules jobs, supplies two isolated checkouts, transports artifacts, and injects
credentials only into execute. Callisto owns source validation, provenance, plan derivation,
provider observation, transition selection, resume, terminality, receipt creation, and artifact
validation. Workflow shell must not reimplement those semantics.

All transient intent, state, downloaded artifacts, and build output live under `RUNNER_TEMP`, not
inside a source checkout. Build and plan have no registry credentials or provider-write token.

## Product assets and rehearsal

The product release will contain attested assets for `aarch64-apple-darwin`,
`x86_64-unknown-linux-gnu`, `x86_64-unknown-linux-musl`, and `wasm32-wasip1`. Apple signing and
notarization are intentionally out of scope until the project has a safe project-owned policy.

Each asset's GitHub provenance is bound to the current coordinator workflow revision: that is the
revision GitHub records for the workflow run. The immutable intent and artifact manifest separately
bind those bytes to the selected release-source revision, so a recovery can safely use current
orchestration without misrepresenting an older source checkout as the workflow source.

The private `orin-dx/callisto-rehearsal` forge repository is isolated from production. It is
reserved for future forge-level experiments; it is not a prerequisite for the current rehearsal.

### Rehearsal boundary

Callisto publishes Cargo crates to crates.io and product assets to GitHub Releases. It does not
publish to AWS, JFrog, GitHub Packages, or another registry merely to simulate a release.

The hermetic provider harness is the rehearsal boundary until the project deliberately adopts an
isolated registry as a product requirement. It exercises the same CLI lifecycle, separate source
worktrees, provider observation, artifact validation, failure recovery, and fresh-runner state
reconstruction without publishing a package or introducing a second distribution service.

The private `orin-dx/callisto-rehearsal` repository remains reserved for future forge-level
experiments. Do not add `[release.profiles.rehearsal]` or registry credentials while the product
has no isolated-registry requirement: a configured profile that cannot prove persistent provider
behavior would create false confidence rather than release safety.
