# Publishing with Callisto

This document covers authentication setup for registry publishing, with particular attention to npm, where there are two distinct auth patterns and common points of confusion.

---

## Local release: `callisto release`

`callisto release` publishes every package whose current version has no tag yet: registry publish, git tag, and GitHub release for each. It runs on any branch and records HEAD's commit as the source.

- `callisto release --dry-run` prints the plan and performs no effect. It works anywhere, including on a dirty worktree or without an `origin` remote (tags are then noted as unbound).
- `--package <ecosystem/name>` (repeatable) restricts the run to named unreleased packages.
- It refuses a dirty worktree: a tracked modification or an untracked file not covered by `.gitignore`. Ignored files never count.
- It prints `Nothing to release.` and exits 0 when every package is already tagged.
- The receipt goes to `--receipt <file>`, else `<state dir>/callisto/<repo-hash>/<intent-digest>/receipt.json` (`$XDG_STATE_HOME`, else `~/.local/state`; `~/Library/Application Support` on macOS). Never inside the checkout.
- A workspace with `[[release.artifact]]` slots or napi/maturin platform packages must release from CI: `callisto release plan`, `release artifact-manifest`, `release execute`.

Before the first effect it checks one credential per operation kind and names the one that is missing:

| Operation | Accepted credential |
|---|---|
| cargo publish | `CARGO_REGISTRY_TOKEN`, or a `cargo login` credentials file |
| npm publish | `NODE_AUTH_TOKEN`, `NPM_TOKEN`, an auth line in `~/.npmrc` or the project `.npmrc`, or OIDC (`ACTIONS_ID_TOKEN_REQUEST_URL`) |
| PyPI publish | `TWINE_PASSWORD`, or OIDC |
| GitHub release | `GH_TOKEN`, `GITHUB_TOKEN`, or `gh auth status` succeeding |

---

## Repository durable-release workflow

Callisto's own `.github/workflows/callisto-release.yml` separates release work into four authority boundaries. A push with pending changesets creates or updates the release PR. That PR versions manifests and changelogs and removes only the changesets it consumed. Nothing is removed from `main` until that PR is merged.

The normal GitHub flow keeps one `callisto/version-packages` release PR and recomputes it from the current base branch whenever changesets land. This is a reconstruction from `main`, not a literal rebase of an old generated commit: recomputation prevents stale version, changelog, or changeset-deletion edits from being carried forward. The action never runs a local `git push` to update that branch. Instead it stages the recomputed change as a commit rooted at the current `main` via GitHub's `createCommitOnBranch` commit API, restricted to non-workflow paths so `.github/workflows/*` is always inherited unchanged from the branch's current tip, then moves the release branch's ref onto that commit with a plain REST ref update. GitHub's built-in `GITHUB_TOKEN` cannot write `.github/workflows/*` through the Git push protocol or through `createCommitOnBranch`'s own file changes on a public repository, but a ref move that carries no workflow-file write is not subject to that restriction, so the built-in token never needs elevated permission to keep the branch current. The resulting commits are GitHub-signed ("Verified") and attributed to `github-actions[bot]`. Repositories that already have a SHA-suffixed branch and PR from before this change (created under the prior fallback behavior) do not need to migrate anything -- that branch is a normal, continuously-updated managed branch going forward. An optional App or fine-grained token remains available for repositories that want release PR operations attributed to a different identity, but is never required.

After a merge, the workflow derives a fresh release run from the exact merged source and pass its immutable handoff between jobs. "Re-run failed jobs" on a failed release run is supported: the rerun adopts every effect that already landed and performs the rest. To release an older merged source with current orchestration, dispatch a new run with that source SHA.

The merged, managed release PR is the sole approval boundary. Do not configure a separate GitHub Environment reviewer gate for this workflow. Keep registry credentials scoped only to the `execute` job, so planning and build jobs cannot read them.

An administrator must also enable a branch-protection rule or ruleset on `main` that requires CODEOWNERS review. [`.github/CODEOWNERS`](../.github/CODEOWNERS) names the real owner for workflow and action changes, but GitHub does not enforce review merely because that file exists.

Only the `execute` job may receive `CARGO_REGISTRY_TOKEN`, `NPM_TOKEN`, or `TWINE_PASSWORD`. Do not put those secrets at workflow scope, in build jobs, or in an action input.

Progress is derived from provider observation (registry, remote tag, forge release, assets), and binary assets are published to the GitHub Release. A green workflow is still not proof that a release exists; use the release receipt and independent provider checks as the completion evidence.

`callisto-action` is now a compatibility version-PR action only. Its former `publish` and `create_github_release` inputs are ignored; it never publishes, tags, downloads artifacts, or creates a forge release. The repository durable workflow is the supported release path.

The binding self-release contract, implementation batches, and cutover evidence are in [`SPEC-SELF-RELEASE-LIFECYCLE`](specs/SPEC-SELF-RELEASE-LIFECYCLE.json) and its [`implementation plan`](projects/SPEC-SELF-RELEASE-LIFECYCLE.json).

---

## npm Registry Authentication

Callisto delegates all npm publishing to the `npm` or `pnpm` CLI (see the coordinator pattern in `00-design.md` §11.1 and §9). This means callisto never reads NPM credentials directly — whatever auth the npm CLI can see when it runs is what gets used. There are two supported patterns.

---

### Pattern A: Manual `.npmrc` setup via `NPM_TOKEN`

Set an `NPM_TOKEN` secret in your repository. Add this only in the protected `execute` job, immediately before the command that publishes:

```yaml
- name: Authenticate with npm registry
  run: npm config set //registry.npmjs.org/:_authToken $NPM_TOKEN
  env:
    NPM_TOKEN: ${{ secrets.NPM_TOKEN }}

- run: callisto release execute --intent "$RUNNER_TEMP/release-intent/release-intent.json" --receipt "$RUNNER_TEMP/release-receipt.json" --orchestration-revision "$GITHUB_SHA"
```

This works because `npm config set` writes to the user-level `.npmrc`, which the npm CLI reads for every subsequent publish call in the same job.

The version-PR action never receives this token. It only creates or updates a reviewed PR.

---

### Pattern B: `actions/setup-node` with `registry-url`

`actions/setup-node` has built-in npm auth support: when you pass `registry-url`, it writes a project-level `.npmrc` configured to read the token from the `NODE_AUTH_TOKEN` environment variable. Set `NODE_AUTH_TOKEN` in the job environment:

```yaml
- uses: actions/setup-node@v4
  with:
    node-version: '20'
    registry-url: 'https://registry.npmjs.org'

- run: callisto release execute --intent "$RUNNER_TEMP/release-intent/release-intent.json" --receipt "$RUNNER_TEMP/release-receipt.json" --orchestration-revision "$GITHUB_SHA"
  env:
    NODE_AUTH_TOKEN: ${{ secrets.NPM_TOKEN }}
```

**How this works:** `actions/setup-node` writes an `.npmrc` file containing a line like: `//registry.npmjs.org/:_authToken=${NODE_AUTH_TOKEN}`. The npm CLI evaluates this shell expansion at publish time, picking up the environment variable. Without the `.npmrc` entry written by `setup-node`, setting `NODE_AUTH_TOKEN` alone has no effect — npm does not read that variable unless a corresponding `.npmrc` line tells it to.

**Warning:** `NODE_AUTH_TOKEN` is silently ignored by npm if no `.npmrc` contains a `${NODE_AUTH_TOKEN}` interpolation. If you set `NODE_AUTH_TOKEN` in your workflow but did not also run `actions/setup-node` with `registry-url` set, your publish will fail with an authentication error — not a variable-not-found error, because npm never reads the variable in that case.

---

### Choosing a pattern

| Criterion | Pattern A (`NPM_TOKEN` + manual step) | Pattern B (`setup-node` + `NODE_AUTH_TOKEN`) |
|---|---|---|
| Depends on `actions/setup-node` | No | Yes — `registry-url` must be set |
| Node.js version management | Separate step or pre-installed | Handled by `setup-node` |
| Recommended when | Your execute job does not otherwise need Node setup | Your execute job already uses `setup-node` |
| Release-PR action receives the token | Never | Never |

If your execute job already calls `actions/setup-node` and sets `registry-url`, Pattern B works because `setup-node` wrote the `.npmrc`. Do not add `NODE_AUTH_TOKEN` to a job that does not run `setup-node` with `registry-url`.

### Provenance

Publish npm packages with provenance from GitHub Actions: give the execute job `permissions: id-token: write` and set `NPM_CONFIG_PROVENANCE: "true"` in its env. npm then attests each package to the workflow run and commit; Callisto needs no flag.

---

## npm Platform/Main Package Publishing

A napi-rs (or maturin) native package publishes as **N platform-specific packages** — one per target triple, e.g. `my-lib-linux-x64-gnu`, `my-lib-darwin-arm64` — plus **one main package** that end users actually install. The main package's `optionalDependencies` pin exact versions of every platform sibling; npm resolves whichever one matches the installer's OS/arch at install time. See `00-design.md` §5.3 and §7.5 for how platform packages are detected from `napi.targets`/`[tool.maturin].targets`.

### Publish order

`callisto release execute` publishes in this fixed order: Rust crates → npm platform packages → npm main packages → PyPI packages. Platform packages always publish before the main package that depends on them.

### What happens when a platform package fails

Each platform publish is a prerequisite of its owner's publish. If one fails, the main package is never attempted, so no `optionalDependencies` point at a version that was never uploaded. Fix the cause and re-run the release: it adopts every effect that already landed and performs the rest.

### What happens when a platform dependency is missing entirely

Planning refuses a selection that leaves out an unreleased platform package a selected npm package depends on (`ReleaseSelectionInvalid`, "select it too"), before anything is published.

---

## Cargo (crates.io) Authentication

Set `CARGO_REGISTRY_TOKEN` as a repository secret and pass it to the job environment. `cargo publish` reads it directly:

```yaml
env:
  CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}
```

No `.cargo/credentials` setup step is needed; cargo recognizes the environment variable natively.

---

## Python (PyPI) Authentication

The durable release (`callisto release plan`/`execute`, see `07-self-release-lifecycle.md`) checks PyPI versions via the PEP 691 JSON simple index.

Python publishing uses `twine upload`. Twine reads credentials from two environment variables: `TWINE_USERNAME` (set to `__token__` when using a PyPI API token) and `TWINE_PASSWORD` (set to the API token value). Set these in the job environment before the Callisto release action runs:

```yaml
env:
  TWINE_USERNAME: __token__
  TWINE_PASSWORD: ${{ secrets.PYPI_TOKEN }}
```

Do not use `TWINE_API_TOKEN` — twine does not recognise that variable name.

---

## Full workflow example with npm auth (Pattern A)

This extends the Paradigm 1 example from `release-paradigms.md` to include npm publishing:

```yaml
name: Release & Publish Workflow

on:
  push:
    branches:
      - main
  workflow_dispatch:

concurrency:
  group: ${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: false

jobs:
  verify:
    name: Verify CI Pipeline
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
      - uses: ./.github/actions/setup-callisto
      - run: moon run :format-check
      - run: moon run :lint
      - run: moon run :test

  release:
    name: Version Packages or Publish Release
    needs: [verify]
    runs-on: ubuntu-latest
    permissions:
      contents: write
      pull-requests: write
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
          token: ${{ secrets.GITHUB_TOKEN }}
      - uses: ./.github/actions/setup-callisto
      - uses: ./.github/actions/callisto-action
        # This action only creates or updates the release PR. Registry tokens
        # belong exclusively in the protected execute job after merge.
```

### Installer verification

`setup-callisto` and `setup-callisto-wasm` verify a downloaded prebuilt asset with `gh attestation verify` (repository `orin-dx/callisto`, signer workflow `.github/workflows/callisto-release.yml`, using the job's `github.token`) before extracting or using it, and extract only the `callisto` binary from the archive. The `verification` input selects the behavior:

- `require`: verification failure or a missing `gh` aborts the step; the asset is never run.
- `fallback` (default): on failure or missing `gh`, warn, delete the asset and install from crates.io (version pinned to the requested tag; unpinned only for `latest`). The unverified asset is never run. For `setup-callisto-wasm`, which has no source-install alternative, `fallback` behaves like `require`.
- `skip`: no verification; warn and use the asset.

Any other value fails immediately. `fallback` is the default because releases built before the first attested release carry no attestation, so `require` would hard-fail existing users; a tampered asset is never run in `require` or `fallback`.
