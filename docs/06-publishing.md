# Publishing with Callisto

This document covers authentication setup for registry publishing, with particular attention
to npm, where there are two distinct auth patterns and common points of confusion.

---

## npm Registry Authentication

Callisto delegates all npm publishing to the `npm` or `pnpm` CLI (see the coordinator
pattern in `00-design.md` §11.1 and §9). This means callisto never reads NPM credentials
directly — whatever auth the npm CLI can see when it runs is what gets used. There are two
supported patterns.

---

### Pattern A: Manual `.npmrc` setup via `NPM_TOKEN` (what `callisto-action` uses)

Set an `NPM_TOKEN` secret in your repository. Add a step before the callisto-action step
that writes the token into the npm config:

```yaml
- name: Authenticate with npm registry
  run: npm config set //registry.npmjs.org/:_authToken $NPM_TOKEN
  env:
    NPM_TOKEN: ${{ secrets.NPM_TOKEN }}

- uses: ./.github/actions/callisto-action
  with:
    publish: "true"
  env:
    GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
    CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}
    NPM_TOKEN: ${{ secrets.NPM_TOKEN }}
```

This works because `npm config set` writes to the user-level `.npmrc`, which the npm CLI
reads for every subsequent publish call in the same job.

`callisto-action` does NOT call `actions/setup-node` internally. If you need npm auth,
Pattern A is the straightforward path that does not depend on any action-provided toolchain
setup.

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

- uses: ./.github/actions/callisto-action
  with:
    publish: "true"
  env:
    GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
    CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}
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
| What the examples in this repo use | Yes (the workflow examples in `release-paradigms.md`) | No |
| Works without callisto-action changes | Yes | Yes, if `setup-node` is already in your job |

The examples in `release-paradigms.md` use Pattern A. If your job already calls
`actions/setup-node` for other reasons and you set `registry-url`, Pattern B works too —
but only because `setup-node` wrote the `.npmrc`. Do not add `NODE_AUTH_TOKEN` to a job
that does not run `setup-node` with `registry-url`.

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
      # Pattern A: write the npm auth token before callisto-action runs.
      # callisto-action does not call actions/setup-node, so NODE_AUTH_TOKEN
      # alone would be silently ignored by npm. This manual step is required.
      - name: Authenticate with npm registry
        run: npm config set //registry.npmjs.org/:_authToken $NPM_TOKEN
        env:
          NPM_TOKEN: ${{ secrets.NPM_TOKEN }}
      - uses: ./.github/actions/callisto-action
        with:
          publish: "true"
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
          CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}
          NPM_TOKEN: ${{ secrets.NPM_TOKEN }}
```
