# Self-release lifecycle

This document is the operational contract for Callisto's own releases. The implementation plan
is [`SPEC-SELF-RELEASE-LIFECYCLE`](projects/SPEC-SELF-RELEASE-LIFECYCLE.json).

## Authority

A verified merge of a managed Callisto release PR is the sole authorization for publication.
There is no GitHub Environment reviewer, deployment protection gate, or second manual click.
Merge evidence and release authorization are separate concerns: required CI checks protect the
merge; the merge authorizes the release.

## Release identity and recovery

Every release run records two different revisions:

- The orchestration revision: the workflow and Callisto CLI that coordinate the run.
- The release-source revision: the merged release commit whose exact versions are released.

A recovery is a new run using current orchestration and an explicit full release-source SHA. It
does not rerun historical workflow code, create a version PR, or invent a newer version.

The immutable run envelope and provider observations are recovery authority. A local state file
is useful crash evidence, but cannot prove what a registry, Git remote, or forge contains.

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

The private `orin-dx/callisto-rehearsal` forge repository is isolated from production. It is not,
by itself, an end-to-end rehearsal: each registry needs a real isolated destination and
credentials. The workflow must fail closed when that infrastructure is not provisioned.

### Rehearsal provisioning boundary

GitHub Packages is not a Cargo registry and must not be selected as the rehearsal destination for
Callisto's Cargo crates. The selected service must expose both a Cargo sparse index and Cargo's
publish API, retain published versions, and provide a scoped token for the rehearsal workflow.
JFrog Artifactory and AWS CodeArtifact document support for that protocol.

Before dispatching the first rehearsal, provision a private Cargo destination distinct from
crates.io, record its credential-free index URL in the rehearsal checkout, and add only its scoped
publish token as a repository secret. Keep `orin-dx/callisto-rehearsal` as the forge destination.
Do not add npm or PyPI credentials unless the rehearsal source actually contains a package routed
to those registries; the current Callisto product release is Cargo-only.

The first rehearsal must use disposable, unreleased versions. It must prove a complete run and a
fresh-runner recovery against the persistent rehearsal providers before any production recovery is
attempted.
