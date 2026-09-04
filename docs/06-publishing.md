# Publishing with Callisto

This document covers authentication setup for registry publishing, with particular attention
to npm, where there are two distinct auth patterns and common points of confusion.

---

## Repository durable-release workflow

Callisto's own `.github/workflows/callisto-release.yml` separates release work into four
authority boundaries. A push with pending changesets creates or updates the release PR. That
PR versions manifests and changelogs and removes only the changesets it consumed. Nothing is
removed from `main` until that PR is merged.

The normal GitHub flow keeps one `callisto/version-packages` release PR and recomputes it from
the current base branch whenever changesets land. This is a reconstruction from `main`, not a
literal rebase of an old generated commit: recomputation prevents stale version, changelog, or
changeset-deletion edits from being carried forward. The action never runs a local `git push`
to update that branch. Instead it stages the recomputed change as a commit rooted at the
current `main` via GitHub's `createCommitOnBranch` commit API, restricted to non-workflow
paths so `.github/workflows/*` is always inherited unchanged from the branch's current tip,
then moves the release branch's ref onto that commit with a plain REST ref update. GitHub's
built-in `GITHUB_TOKEN` cannot write `.github/workflows/*` through the Git push protocol or
through `createCommitOnBranch`'s own file changes on a public repository, but a ref move that
carries no workflow-file write is not subject to that restriction, so the built-in token never
needs elevated permission to keep the branch current. The resulting commits are GitHub-signed
("Verified") and attributed to `github-actions[bot]`. Repositories that already have a
SHA-suffixed branch and PR from before this change (created under the prior fallback behavior)
do not need to migrate anything -- that branch is a normal, continuously-updated managed branch
going forward. An optional App or fine-grained token remains available for repositories that
want release PR operations attributed to a different identity, but is never required.

After a merge, the workflow derives a transient release intent from the signed merge commit's
actual delta, builds from that exact commit, and passes the intent between jobs as a same-run
GitHub artifact with a SHA-256 sidecar. It is not committed and it is never recovered from a
cache. The execute job rechecks the handoff before calling `callisto release execute`.

Create a GitHub Environment named `release` before enabling registry credentials. Configure
at least one required reviewer and appropriate branch/tag deployment rules in the repository's
Environment settings. The workflow queries the Environment API and fails if the reviewer rule
is absent; the `environment: release` job boundary is what prevents registry secrets from being
available to planning or build jobs.

An administrator must also enable a branch-protection rule or ruleset on `main` that requires
CODEOWNERS review. [`.github/CODEOWNERS`](../.github/CODEOWNERS) names the real owner for
workflow and action changes, but GitHub does not enforce review merely because that file exists.

Only the `execute` job may receive `CARGO_REGISTRY_TOKEN`, `NPM_TOKEN`, or `TWINE_PASSWORD`.
Do not put those secrets at workflow scope, in build jobs, or in an action input. Binary releases
add exact files plus `release-artifacts/manifest.json` to the build handoff; their GitHub build
attestations are verified by Callisto before upload. Source-only releases do not need that
manifest.

`callisto-action` is now a compatibility version-PR action only. Its former `publish` and
`create_github_release` inputs are ignored; it never publishes, tags, downloads artifacts, or
creates a forge release. The repository durable workflow is the supported release path.

---

## npm Registry Authentication

Callisto delegates all npm publishing to the `npm` or `pnpm` CLI (see the coordinator
pattern in `00-design.md` §11.1 and §9). This means callisto never reads NPM credentials
directly — whatever auth the npm CLI can see when it runs is what gets used. There are two
supported patterns.

---

### Pattern A: Manual `.npmrc` setup via `NPM_TOKEN`

Set an `NPM_TOKEN` secret in your repository. Add this only in the protected `execute`
job, immediately before the command that publishes:

```yaml
- name: Authenticate with npm registry
  run: npm config set //registry.npmjs.org/:_authToken $NPM_TOKEN
  env:
    NPM_TOKEN: ${{ secrets.NPM_TOKEN }}

- run: callisto release execute --intent .release-intent/release-intent.json
```

This works because `npm config set` writes to the user-level `.npmrc`, which the npm CLI
reads for every subsequent publish call in the same job.

The version-PR action never receives this token. It only creates or updates a reviewed PR.

---

### Pattern B: `actions/setup-node` with `registry-url`

`actions/setup-node` has built-in npm auth support: when you pass `registry-url`, it writes
a project-level `.npmrc` configured to read the token from the `NODE_AUTH_TOKEN` environment
variable. Set `NODE_AUTH_TOKEN` in the job environment:

```yaml
- uses: actions/setup-node@v4
  with:
    node-version: '20'
    registry-url: 'https://registry.npmjs.org'

- run: callisto release execute --intent .release-intent/release-intent.json
  env:
    NODE_AUTH_TOKEN: ${{ secrets.NPM_TOKEN }}
```

**How this works:** `actions/setup-node` writes an `.npmrc` file containing a line like:
`//registry.npmjs.org/:_authToken=${NODE_AUTH_TOKEN}`. The npm CLI evaluates this shell
expansion at publish time, picking up the environment variable. Without the `.npmrc` entry
written by `setup-node`, setting `NODE_AUTH_TOKEN` alone has no effect — npm does not read
that variable unless a corresponding `.npmrc` line tells it to.

**Warning:** `NODE_AUTH_TOKEN` is silently ignored by npm if no `.npmrc` contains a
`${NODE_AUTH_TOKEN}` interpolation. If you set `NODE_AUTH_TOKEN` in your workflow but did
not also run `actions/setup-node` with `registry-url` set, your publish will fail with an
authentication error — not a variable-not-found error, because npm never reads the variable
in that case.

---

### Choosing a pattern

| Criterion | Pattern A (`NPM_TOKEN` + manual step) | Pattern B (`setup-node` + `NODE_AUTH_TOKEN`) |
|---|---|---|
| Depends on `actions/setup-node` | No | Yes — `registry-url` must be set |
| Node.js version management | Separate step or pre-installed | Handled by `setup-node` |
| Recommended when | Your execute job does not otherwise need Node setup | Your execute job already uses `setup-node` |
| Release-PR action receives the token | Never | Never |

If your execute job already calls `actions/setup-node` and sets `registry-url`, Pattern B
works because `setup-node` wrote the `.npmrc`. Do not add `NODE_AUTH_TOKEN` to a job that
does not run `setup-node` with `registry-url`.

---

## npm Platform/Main Package Publishing

A napi-rs (or maturin) native package publishes as **N platform-specific packages** — one
per target triple, e.g. `my-lib-linux-x64-gnu`, `my-lib-darwin-arm64` — plus **one main
package** that end users actually install. The main package's `optionalDependencies` pin
exact versions of every platform sibling; npm resolves whichever one matches the installer's
OS/arch at install time. See `00-design.md` §5.3 and §7.5 for how platform packages are
detected from `napi.targets`/`[tool.maturin].targets`.

### Publish order

`callisto publish` publishes in this fixed order: Rust crates → npm platform packages → npm
main packages → PyPI packages. Platform packages always publish before the main package that
depends on them.

### What happens when a platform package fails

If a platform package fails to publish in the same run, its dependent main package is
**skipped, not attempted** — publishing it anyway would ship `optionalDependencies` pointing
at a version that was never uploaded. The skipped entry appears in the publish report as:

```json
{
  "package": "npm/my-lib",
  "status": "failed",
  "errorKind": "dependencyFailed",
  "error": "skipped: platform dependency failed to publish: my-lib-linux-x64-gnu"
}
```

`dependencyFailed` means exactly this — the main package was never attempted, because a
declared platform dependency failed to publish *in this same run*. It is not itself a
registry error. **Remediation:** fix whatever made the platform package fail, then re-run
`callisto publish` — it is idempotent and will skip anything already uploaded.

### What happens when a platform dependency is missing entirely

Before any registry call is made, `callisto plan-publish` cross-checks every main package's
declared platform dependencies against what actually ended up in the plan. A dependency is
allowed to be absent only if it's already published (its on-disk version already matches its
last release tag). Otherwise — a misconfigured platform package with no npm publish target,
or one excluded via `--package` — planning fails outright with a `MissingPlatformDependency`
error naming the main package and the missing dependency, before anything is published.

---

## Cargo (crates.io) Authentication

Set `CARGO_REGISTRY_TOKEN` as a repository secret and pass it to the job environment.
`cargo publish` reads it directly:

```yaml
env:
  CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}
```

No `.cargo/credentials` setup step is needed; cargo recognizes the environment variable
natively.

---

## Python (PyPI) Authentication

Python publishing uses `twine upload`. Twine reads credentials from two environment
variables: `TWINE_USERNAME` (set to `__token__` when using a PyPI API token) and
`TWINE_PASSWORD` (set to the API token value). Set these in the job environment before
the Callisto release action runs:

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
