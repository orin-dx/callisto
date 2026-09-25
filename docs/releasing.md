# Releasing

How a Callisto-managed release moves from changeset to published package. The contract is [`specs/release.json`](specs/release.json).

For registry credential setup (npm, crates.io, PyPI), see [`publishing.md`](publishing.md).

---

## Local release: `callisto release`

`callisto release` publishes every package whose current version has no tag yet: registry publish, git tag, and GitHub release for each. It runs on any branch and records HEAD's commit as the source.

- `callisto release --dry-run` prints the plan and performs no effect. It works anywhere, including on a dirty worktree or without an `origin` remote (tags are then noted as unbound).
- `--package <ecosystem/name>` (repeatable) restricts the run to named unreleased packages, plus every member of their fixed or linked groups. An unreleased workspace package that a selected one depends on (runtime, optional, or peer) must be selected too.
- It refuses a dirty worktree: a tracked modification or an untracked file not covered by `.gitignore`. Ignored files never count.
- It prints `Nothing to release.` and exits 0 when every package is already tagged.
- It prints a summary. With `--receipt <file>` it also writes the receipt there, only when every operation succeeded; after a partial failure, rerun to adopt what landed.
- A workspace with `[[release.artifact]]` slots or napi/maturin platform packages must release from CI: `callisto release plan`, `release artifact-manifest`, `release execute` (all CI-internal — `#[command(hide = true)]`, not meant for interactive use).

Callisto does not check registry credentials itself. `cargo`, `npm`, `twine`, and `gh` each report their own auth failure; fix it and rerun, which adopts every effect that already landed.

---

## Generated workflows

`callisto init --workflow` (or answering yes to its prompt) writes `.github/workflows/callisto-release.yml`. That path is what `release plan` binds artifact attestations to — keep the name if you rename anything else.

- Init never overwrites an existing file.
- This repo's own `.github/workflows/callisto-release.yml` is a worked example.

- **Simple shape** (no `[[release.artifact]]` slots, no napi platform packages): a `version-pr` job keeps the release PR current, and a `release` job runs `callisto release` on every push to the default branch. That is a no-op until the merged release PR leaves an untagged version.
- **Build-matrix shape** (artifact slots or napi platform packages): `version-pr`, then `plan` -> `build` -> `execute`.
  - `plan` runs only when the pushed commit writes `.callisto/release-decision.json`.
  - `build` has one matrix job per napi target from `callisto matrix` and one per artifact slot, and attests the slot assets.
  - `execute` places the napi builds (`napi artifacts`), writes the artifact manifest, and runs `callisto release execute`.
- Maturin builds, and platform packages without `napi.targets`, get no generated workflow.

The default branch is the approval boundary: a merge to it publishes. Protect it with a rule that requires pull requests and reviews (GitHub -> Settings -> Rules). Init prints this reminder after writing the file.

Limits of the build-matrix shape:

- `napi build` runs in the napi package's directory. When the addon crate lives elsewhere (for example `packages/napi` with the crate in `crates/binding`), the step passes `--manifest-path` for the workspace crate whose lib name equals `napi.binaryName`; `callisto matrix` fails with E204 when no single crate matches.
- `napi artifacts` writes `.node` files into the checkout, and `release execute` refuses a dirty worktree, so `*.node` must be gitignored.
- Artifact slots build the package's `bin` targets with `cargo build --release --locked`. Any other build (a `cdylib`, custom features) needs a hand-edited step.

---

## This repo's durable-release workflow

Callisto's own `.github/workflows/callisto-release.yml` separates release work into four authority boundaries. A push with pending changesets creates or updates the release PR. That PR versions manifests and changelogs and removes only the changesets it consumed. Nothing is removed from `main` until that PR is merged.

One `callisto/version-packages` release PR is kept current, recomputed from `main` whenever changesets land — a reconstruction, not a rebase of an old commit, so stale edits are never carried forward.

- The action never runs a local `git push` to update that branch. It stages the recomputed change via GitHub's `createCommitOnBranch` commit API, then moves the release branch's ref with a plain REST ref update.
- `createCommitOnBranch` is restricted to non-workflow paths, so `.github/workflows/*` is always inherited unchanged from the branch's current tip. `GITHUB_TOKEN` cannot write `.github/workflows/*` through either the Git push protocol or that API's own file changes on a public repository — but a ref move carrying no workflow-file write isn't subject to that restriction, so the built-in token never needs elevated permission.
- Resulting commits are GitHub-signed ("Verified"), attributed to `github-actions[bot]`. An optional App or fine-grained token remains available for release-PR operations attributed to a different identity, but is never required.

After a merge, the workflow derives a fresh release run from the exact merged source and passes its immutable handoff between jobs. "Re-run failed jobs" on a failed release run is supported: the rerun adopts every effect that already landed and performs the rest. To release an older merged source with current orchestration, dispatch a new run with that source SHA (see Recovery below).

Callisto treats the verified merge of the release PR as the authorization and has no approval step of its own; any extra gate is your CI's choice. Keep registry credentials scoped only to the `execute` job, so planning and build jobs cannot read them. Only `execute` may receive `CARGO_REGISTRY_TOKEN`, `NPM_TOKEN`, or `TWINE_PASSWORD` — never at workflow scope, in build jobs, or in an action input.

An administrator must also enable a branch-protection rule or ruleset on `main` that requires CODEOWNERS review; `.github/CODEOWNERS` names the owner for workflow and action changes, but GitHub does not enforce review merely because that file exists.

Progress is derived from provider observation (registry, remote tag, forge release, assets); binary assets publish to the GitHub Release. A green workflow is not by itself proof a release exists — use the release receipt and independent provider checks as completion evidence.

`callisto-action` has two modes: `mode: version-pr` (default) opens or updates the release PR; `mode: release` installs callisto and runs `callisto release`. Its former `publish` and `create_github_release` inputs are ignored. Workspaces with artifact slots or platform packages release through the plan/build/execute workflow instead.

### `callisto-action` inputs

[`.github/actions/callisto-action/action.yml`](../.github/actions/callisto-action/action.yml):

| Input | Default | Purpose |
| :--- | :--- | :--- |
| `version_command` | `callisto version --refresh-lockfiles` | Versioning command; `--emit-decision <decision_path>` is appended automatically. |
| `decision_path` | `.callisto/release-decision.json` | Where the version command records the exact release decision the PR carries; `release plan --from-release-commit` verifies the merged commit against this file. |
| `commit_message` | `chore(release): version packages` | Release-PR commit message. |
| `title` | `chore(release): version packages` | Release-PR title. |
| `pr_label` | `callisto: release` | Release-PR label. |
| `setup_git_user` | `true` | Unused no-op, kept for backward compatibility. The managed branch is committed through the forge commit API, not a local `git commit`. |
| `branch` | `main` | Base branch for the release PR. |
| `release_branch` | `callisto/version-packages` | Managed head branch for the release PR. |
| `github_token` | `""` | Optional token for PR and forge commit API operations. The default `GITHUB_TOKEN` is sufficient even on a public repository, since the executor never writes `.github/workflows/*`. |
| `setup_callisto` | `true` | Install the Callisto environment before running. |
| `cwd` | `.` | Workspace directory. |
| `mode` | `version-pr` | `version-pr`: create/update the release PR. `release`: run `callisto release`. |

Outputs: `hasChangesets` (`version-pr` mode only), `published` and `publishedPackages` (`release` mode only; otherwise `false`/`[]`).

## PR pre-flight verification

[`.github/actions/callisto-validate/action.yml`](../.github/actions/callisto-validate/action.yml) is a reusable action for your PR workflow:

- `callisto status --check` — one gate covering config health, package discovery, and changeset syntax: exit 0 with no error-level diagnostics, exit 1 otherwise.
- `callisto release --dry-run --format text` — simulates the release plan without effect.
- Writes a workspace status summary to the job's GitHub Actions summary page.

---

## Authority

A verified merge of a managed Callisto release PR authorizes publication; callisto adds no approval step. Protect the merge with required reviews and CI checks on the default branch.

## Release identity and recovery

Every release run records two different revisions:

- The **orchestration revision**: the workflow and Callisto CLI that coordinate the run.
- The **release-source revision**: the merged release commit whose exact versions are released.

A recovery is a new run using current orchestration and an explicit full release-source SHA. It does not rerun historical workflow code, create a version PR, or invent a newer version.

The orchestration revision is the SHA of the run itself (`github.sha`), never the moving tip of `main`, because attestations are stamped with the run's SHA. The release path runs on `main` only. A recovery dispatch is gated by `recovery-checks` (credential-free release workflow contract and policy checks), not the full `verify` job, so an unrelated failure on `main` cannot block publishing an already-merged release.

A local `callisto release` (no artifact slots, no platform packages) is its own orchestration: both revisions are HEAD's commit, and it may run on a branch. The CI route (`release plan`/`release execute`) still requires the exact merge commit checked out detached.

Every run is the same kind of run.

- A push-triggered release whose `execute` job fails partway is fixed by "Re-run failed jobs": the rerun starts from nothing, observes each provider, adopts every operation that is already exactly done (printing one warning line per registry version it skips), and performs the rest.
- A different object at the same identity is a conflict, never adopted.
- Execution state is in memory only — nothing is persisted between runs.

The run envelope (orchestration revision, release-source revision, intent digest, artifact-manifest digest) is derived from the intent by one constructor, so source revision and intent digest have no second authority. It is validated across fields before the first effect. The receipt is built from that envelope plus the exact provider evidence each operation recorded when it succeeded; nothing is re-observed after the effects.

Provider observations are the only authority on what a registry, Git remote, or forge contains.

`[release].forge-repository` in `callisto.toml` is the one GitHub forge destination. Planning rejects an `--artifact-repository` that differs from it, execution rejects an intent whose artifact slots target another repository, and a `[release]` without it fails before writing an intent. A legacy `[release.profiles.production].forge-repository` is still read when the top-level key is absent, for migration only; any other profile name is rejected.

Each publish target resolves its own registry key (`cratesIo`, `npm`, `pypi`, `nuget`) against `[registries.<key>]` (see `config.md`), or the built-in default when unconfigured.

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

Before an effect, Callisto observes the provider. An operation is one of absent, exact success, conflict, or indeterminate.

- Exact success carries typed per-role evidence: registry version with checksum and yanked flag, tag peeled commit, forge release tag and draft flag, asset size and sha256.
- Conflict reasons and indeterminate causes are closed enums; an authentication, rate-limit, or transport failure is always indeterminate, never a conflict.
- Exact success becomes `AlreadySatisfied` — Callisto never publishes the same version merely to learn whether it exists.
- Conflict and indeterminate observations fail closed with an actionable diagnostic.

A single pure transition table is the only way an operation's state changes. `Attempting` is reachable only through an absent proof; "our attempt landed" (`Published`) is distinct from "it already existed" (`AlreadySatisfied`).

Registry observation goes through the ecosystem's own package manager — the same tool and configuration the publish effect uses.

- **Cargo**: `cargo info NAME@VERSION --registry REGISTRY`, run from the source workspace root so `.cargo/config.toml` registry definitions and credentials apply (`--registry` is never omitted). Exit 0 with a matching `version:` line is exact evidence; exit 101 with ``could not find `NAME@VERSION` `` is absence; anything else (including an unreachable registry) is indeterminate and retried. A yanked version reads as absent, because `cargo info` cannot see yanks — that fails closed: the registry refuses the publish that follows, and the run ends in a typed unconfirmed-publication error rather than a receipt.
- **npm**: `npm view NAME@VERSION version --json`.
- **PyPI**: checked with `curl` against the PEP 691 JSON simple index (`https://pypi.org/simple/<project>/` or the configured private index), not `pip`, which can't tell a missing project from an unreachable index. A matching file is exact; 404 or no match is absent; a transport failure or non-JSON (PEP 503 HTML) response is indeterminate. Yanked files count as absent, so publishing over a yanked version fails closed.

Registry endpoints must be `https` — no loopback exception, since an endpoint receives a credential.

One bounded retry policy covers read-only observations only (registry query, GitHub release lookup, `git ls-remote`): HTTP 429, 5xx, a rate-limited 403, and command/transport failures retry with backoff, honoring `Retry-After`. Effects are never blindly re-issued. Every outbound command has a deadline.

The GitHub Release is created as a draft, assets are uploaded to the draft, and publishing is a separate last operation depending on every upload, because a published release can be immutable and refuse further assets.

The release succeeds only when every operation is provider-observed as exact success or already satisfied. `Attempting`, failed, blocked, missing, and indeterminate are non-success states. A receipt is emitted only after that global condition holds.

## Workflow boundary

GitHub Actions schedules jobs, supplies two isolated checkouts, transports artifacts, and injects credentials only into `execute`. Callisto owns source validation, provenance, plan derivation, provider observation, transition selection, terminality, receipt creation, and artifact validation. Workflow shell must not reimplement those semantics.

All transient intent, receipt, downloaded artifacts, and build output live under `RUNNER_TEMP`, not inside a source checkout. Build and plan have no registry credentials or provider-write token.

## Product assets and rehearsal

The product release contains attested assets for `aarch64-apple-darwin`, `x86_64-unknown-linux-gnu`, and `x86_64-unknown-linux-musl`. Apple signing and notarization are intentionally out of scope until the project has a safe project-owned policy.

Each asset's GitHub provenance is bound to the current coordinator workflow revision — the revision GitHub records for the workflow run. The immutable intent and artifact manifest separately bind those bytes to the selected release-source revision, so a recovery can safely use current orchestration without misrepresenting an older source checkout as the workflow source.

A source without a `[release]` section plans with zero artifact slots and prints a notice.

`previous-tag-templates` in `callisto.toml` lets a package's tag-naming convention change without losing continuity with its prior release tags.

The `setup-callisto` action verifies a downloaded prebuilt asset with `gh attestation verify` against `orin-dx/callisto` and the `callisto-release.yml` signer workflow, and never runs an unverified asset unless `verification: skip` (modes: `require`, `fallback` default, `skip`).

Callisto publishes Cargo crates to crates.io and product assets to GitHub Releases only — it does not publish to another registry merely to simulate a release. The provider contract tier (`testing/fixtures/providers`, `just provider-fixtures`, `just provider-contract`) keeps fake providers honest against captured real responses; a deterministic fault-injection simulator drives the real release executor against an in-memory provider world, crashing at every provider call and injecting each outcome, then asserts the lifecycle invariants after each run.

---

## Platform-package publish order

`callisto release execute` publishes in this fixed order: Rust crates -> npm platform packages -> npm main packages -> PyPI packages. Platform packages always publish before the main package that depends on them.

A napi-rs (or maturin) native package publishes as N platform-specific packages (one per target triple) plus one main package end users install; the main package's `optionalDependencies` pin exact versions of every platform sibling.

- **A platform package fails**: its owner's main package is never attempted, so no `optionalDependencies` point at a version that was never uploaded. Fix the cause and re-run — it adopts every effect that already landed.
- **A platform dependency is missing from the selection entirely**: planning refuses (`ReleaseSelectionInvalid`, "select it too") before anything is published.
