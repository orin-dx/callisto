# Self-release lifecycle

This document is the operational contract for Callisto's own releases. The implementation plan is [`SPEC-SELF-RELEASE-LIFECYCLE`](projects/SPEC-SELF-RELEASE-LIFECYCLE.json).

## Authority

A verified merge of a managed Callisto release PR is the sole authorization for publication. There is no GitHub Environment reviewer, deployment protection gate, or second manual click. Merge evidence and release authorization are separate concerns: required CI checks protect the merge; the merge authorizes the release.

## Merge evidence

The active `main` ruleset requires one approving review, signed commits, linear squash history, and the following named evidence before a release PR can merge:

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

The repository owner has an explicit ruleset bypass for emergency recovery. That bypass is not a second approval mechanism and must be used only when the normal evidence path cannot run.

## Release identity and recovery

Every release run records two different revisions:

- The orchestration revision: the workflow and Callisto CLI that coordinate the run.
- The release-source revision: the merged release commit whose exact versions are released.

A recovery is a new run using current orchestration and an explicit full release-source SHA. It does not rerun historical workflow code, create a version PR, or invent a newer version.

The orchestration revision is the SHA of the run itself (`github.sha`), never the moving tip of `main`, because attestations are stamped with the run's SHA. The release path runs on `main` only. A recovery dispatch is gated by `recovery-checks` (credential-free release workflow contract and policy checks), not the full `verify` job, so an unrelated failure on `main` cannot block publishing an already merged release.

Every run is the same kind of run. A push-triggered release whose `execute` job fails partway is fixed by "Re-run failed jobs": the rerun starts from nothing, observes each provider, adopts every operation that is already exactly done (printing one warning line per registry version it skips), and performs the rest. A different object at the same identity is a conflict, never adopted. Execution state is in memory only; nothing is persisted between runs.

The run envelope (orchestration revision, release-source revision, profile, intent digest, artifact-manifest digest) is derived from the intent by one constructor, so profile, source revision, and intent digest have no second authority. It is validated across fields (source revision equals the intent's Git source; the manifest digest is present exactly when the intent declares artifact slots; the orchestration revision equals every slot's attestation workflow commit) before the first effect. The receipt is built from that envelope plus the exact provider evidence each operation recorded when it succeeded; nothing is re-observed after the effects.

Provider observations are the only authority on what a registry, Git remote, or forge contains.

The selected release profile is part of the immutable intent digest, not just a receipt label. The production profile declares its GitHub forge destination in `callisto.toml`; planning rejects an artifact repository that differs from that destination, and execution rejects a profile that does not match its intent. An unconfigured profile fails before it writes an intent or dispatches an effect.

Each configured profile also declares `registry-routes`, mapping a logical package target such as `cratesIo` to a concrete configured registry key. The resolved key and its credential-free endpoint digest are part of the release operation and package fingerprint. A configured profile cannot fall back to an un-routed production registry, and two profiles cannot share a forge or configured registry destination.

## Recovering an older release

Recovering a commit that is no longer a branch tip needs two manual steps.

**E180: the workflow can't push tags.** GitHub rejects a `GITHUB_TOKEN` tag push when the tagged commit's workflows differ from every branch tip. Push every tag of the release yourself (PAT or deploy key), then dispatch the release for that SHA:

```sh
SHA=<release-source sha>
git tag -a "<tag>" -m "Release <tag>" "$SHA"   # each tag of the release
git push origin "<tag>"
gh workflow run callisto-release.yml --ref main -f release_source_sha="$SHA"
```

**`ArtifactAssetDiffers`: rebuilt assets don't match.** Rebuilt tarballs aren't byte-identical. Delete the mismatched assets and dispatch again:

```sh
gh release delete-asset "<release tag>" "<asset>" --yes   # each mismatched asset
gh workflow run callisto-release.yml --ref main -f release_source_sha="$SHA"
```

## Execution

Before an effect, Callisto observes the provider. An operation is one of absent, exact success, conflict, or indeterminate. Exact success carries typed per-role evidence: registry version with checksum and yanked flag, tag peeled commit, forge release tag and draft flag, asset size and sha256. Conflict reasons and indeterminate causes are closed enums; an authentication, rate-limit, or transport failure is always indeterminate, never a conflict. Exact success becomes `AlreadySatisfied`; Callisto must not publish the same version merely to learn whether it exists. Conflict and indeterminate observations fail closed with an actionable diagnostic.

A single pure transition table is the only way an operation's state changes. `Attempting` is reachable only through an absent proof; "our attempt landed" (`Published`) is distinct from "it already existed" (`AlreadySatisfied`).

Registry observation goes through the ecosystem's own package manager — the same tool and the same configuration the publish effect uses, so registry resolution and authentication (private registries included) are the tool's job rather than something Callisto reimplements. For cargo it is `cargo info NAME@VERSION --registry REGISTRY`, run from the source workspace root so the workspace's `.cargo/config.toml` registry definitions and credentials apply; `--registry` is never omitted, because without it `cargo info` answers from the local manifest. Exit 0 with a matching `version:` line is exact evidence (with no checksum and no yank claim: `cargo info` reports neither); exit 101 with ``could not find `NAME@VERSION` `` is an absence; anything else, an unreachable registry included, is indeterminate and retried. For npm it is `npm view NAME@VERSION version --json`.

A yanked version reads as absent, because `cargo info` cannot see yanks. That fails closed: the publish that follows is refused by the registry, and since only an exact observation may satisfy an operation, the run ends in the typed unconfirmed-publication error rather than in a receipt.

PyPI versions are checked with `curl` against the PEP 691 JSON simple index (`https://pypi.org/simple/<project>/` or the configured private index), not `pip`, which can't tell a missing project from an unreachable index. A matching file is exact; 404 or no match is absent; a transport failure or non-JSON (PEP 503 HTML) response is indeterminate. Yanked files count as absent, so publishing over a yanked version fails closed.

Registry endpoints must be `https`. There is no loopback exception, because an endpoint receives a credential.

One bounded retry policy covers read-only observations only (the registry query, the GitHub release lookup, `git ls-remote`): HTTP 429, 5xx, a rate-limited 403, and command/transport failures are retried with backoff, honouring `Retry-After`. Effects are never blindly re-issued. Every outbound command has a deadline.

The GitHub Release is created as a draft, assets are uploaded to the draft, and publishing is a separate last operation that depends on every upload, because a published release can be immutable and refuse further assets.

The release succeeds only when every operation is provider-observed as exact success or already satisfied. `Attempting`, failed, blocked, missing, and indeterminate are non-success states. A receipt is emitted only after that global condition holds.

## Workflow boundary

GitHub Actions schedules jobs, supplies two isolated checkouts, transports artifacts, and injects credentials only into execute. Callisto owns source validation, provenance, plan derivation, provider observation, transition selection, terminality, receipt creation, and artifact validation. Workflow shell must not reimplement those semantics.

All transient intent, receipt, downloaded artifacts, and build output live under `RUNNER_TEMP`, not inside a source checkout. Build and plan have no registry credentials or provider-write token.

## Product assets and rehearsal

The product release will contain attested assets for `aarch64-apple-darwin`, `x86_64-unknown-linux-gnu`, `x86_64-unknown-linux-musl`, and `wasm32-wasip1`. Apple signing and notarization are intentionally out of scope until the project has a safe project-owned policy.

Each asset's GitHub provenance is bound to the current coordinator workflow revision: that is the revision GitHub records for the workflow run. The immutable intent and artifact manifest separately bind those bytes to the selected release-source revision, so a recovery can safely use current orchestration without misrepresenting an older source checkout as the workflow source.

A source without a `[release]` section plans with zero artifact slots and prints a notice.

The `callisto@{version}` product tag replaced `callisto-cli@{version}`; `previous-tag-templates` in `callisto.toml` keeps tags from the old template discoverable as the last release.

The `setup-callisto` and `setup-callisto-wasm` actions verify a downloaded prebuilt asset with `gh attestation verify` against `orin-dx/callisto` and the `callisto-release.yml` signer workflow, and never run an unverified asset unless `verification: skip` (modes: `require`, `fallback` default, `skip`).

### Rehearsal boundary

Callisto publishes Cargo crates to crates.io and product assets to GitHub Releases. It does not publish to AWS, JFrog, GitHub Packages, or another registry merely to simulate a release.

The provider contract tier keeps the fakes honest: fake providers serve captured real responses (`testing/fixtures/providers`, provenance recorded per provider), every fake rejects flags and subcommands outside the real tools' shapes, and a test checks each emitted flag against the real tool's own help. `just provider-fixtures` refreshes fixtures and `just provider-contract` re-fetches live to assert shapes still match; both are manual and network-dependent.

A deterministic fault-injection simulator drives the real release executor against an in-memory provider world, crashing at every provider call and injecting each provider outcome, then asserts the lifecycle invariants after each run.

The hermetic provider harness is the rehearsal boundary until the project deliberately adopts an isolated registry as a product requirement. It exercises the same CLI lifecycle, separate source worktrees, provider observation, artifact validation, and fresh reruns that adopt landed effects without publishing a package or introducing a second distribution service.

The private `orin-dx/callisto-rehearsal` repository remains reserved for future forge-level experiments. Do not add `[release.profiles.rehearsal]` or registry credentials while the product has no isolated-registry requirement: a configured profile that cannot prove persistent provider behavior would create false confidence rather than release safety.
