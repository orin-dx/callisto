# Callisto Monorepo Release Paradigms & CI/CD Safety

Callisto supports three release paradigms. Each gates publishing behind CI verification, but they differ in branching model and how much manual coordination they require.

---

## Overview Matrix

| Paradigm | Branching Model | Trigger Mechanism | Developer Overhead | Safety Assurance |
| :--- | :--- | :--- | :--- | :--- |
| **Paradigm 1 (Default)** | Single Trunk (`main`) | Version PR Merge + Gated CI | Low (Automated) | High (`needs: [verify]`) |
| **Paradigm 2** | Dedicated Release (`release`) | Branch Sync / Fast-Forward | Medium (Branch Management) | High (Isolated Branch) |
| **Paradigm 3** | Git Tag-Driven | Tag Push (`*@*`, `v*`) | Medium (Tag Orchestration) | High (Immutable Commit SHA) |

---

## Paradigm 1: Single Trunk (`main`) + Gated CI Verification (Default)

### Architecture
All development lands on `main`. Callisto automatically generates a Version PR (`callisto/version-packages`) when changesets are present. Merging the Version PR to `main` triggers a two-stage release workflow:

1. **Job 1 (`verify`)**: Executes full workspace verification (`moon run :format-check`, `moon run :lint`, `moon run :test`, WASM compilation, and `cargo-deny` security audit).
2. **Job 2 (`release`)**: Depends on Job 1 (`needs: [verify]`). If any test or check fails in Job 1, GitHub Actions blocks Job 2 from running.

### Example Workflow Configuration (`.github/workflows/callisto-release.yml`)

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
      - uses: ./.github/actions/setup-callisto-wasm
      - run: moon run :format-check
      - run: moon run :lint
      - run: moon run :test
      - run: cargo check -p callisto-moon --target wasm32-wasip1 --features pdk
      - run: cargo-deny check

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
      # npm auth: callisto-action does NOT call actions/setup-node, so NODE_AUTH_TOKEN
      # is silently ignored by npm. Use Pattern A: write the token to npm config explicitly.
      # See docs/06-publishing.md for a full explanation of Pattern A vs. Pattern B.
      - name: Authenticate with npm registry
        run: npm config set //registry.npmjs.org/:_authToken $NPM_TOKEN
        env:
          NPM_TOKEN: ${{ secrets.NPM_TOKEN }}
      - uses: ./.github/actions/callisto-action
        with:
          # Publishing is performed by the durable repository workflow after merge.
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
          CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}
          NPM_TOKEN: ${{ secrets.NPM_TOKEN }}
```

---

## Paradigm 2: Dedicated Release Branch (`release` or `stable`)

### Architecture
Development occurs on `main`. Publishing is strictly disabled on `main`. Production releases only execute when commits land on a protected `release` (or `stable`) branch.

1. Developers push PRs to `main`.
2. When ready for a production release, `main` is merged or fast-forwarded into `release`.
3. Callisto generates Version PRs targeting `release` via `callisto compose-pr-body --branch release`.
4. The release workflow executes only on the `release` branch.

### Example Workflow Configuration

```yaml
name: Release & Publish Workflow

on:
  push:
    branches:
      - release
  workflow_dispatch:

jobs:
  verify:
    name: Verify Release Branch
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: ./.github/actions/setup-callisto
      - run: moon run :test

  release:
    name: Publish Production Release
    needs: [verify]
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          token: ${{ secrets.GITHUB_TOKEN }}
      - uses: ./.github/actions/setup-callisto
      - name: Authenticate with npm registry
        run: npm config set //registry.npmjs.org/:_authToken $NPM_TOKEN
        env:
          NPM_TOKEN: ${{ secrets.NPM_TOKEN }}
      - uses: ./.github/actions/callisto-action
        with:
          # Publishing is performed by the durable repository workflow after merge.
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
          CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}
          NPM_TOKEN: ${{ secrets.NPM_TOKEN }}
```

---

## Paradigm 3: Git Tag-Driven Publishing (`v*` or `@scope/pkg@*`)

### Architecture
Commits to branches never trigger package publishing. Publishing is triggered exclusively when a signed Git tag matching a package tag template (e.g. `callisto-cli@0.2.0` or `v1.0.0`) is pushed.

1. Developers run `callisto version` to update package manifests and changelogs.
2. Maintainers push specific release tags to GitHub (`git push origin callisto-cli@0.2.0`).
3. Callisto's tag resolution engine extracts the target package and version from `$GITHUB_REF_NAME`.
4. The workflow verifies the exact commit SHA anchored by the tag before publishing.

### Example Workflow Configuration

```yaml
name: Tag Release Workflow

on:
  push:
    tags:
      - '*@*'
      - 'v*'

jobs:
  verify:
    name: Verify Tagged Commit
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: ./.github/actions/setup-callisto
      - run: moon run :test

  release:
    name: Publish Tagged Package
    needs: [verify]
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: ./.github/actions/setup-callisto
      - name: Authenticate with npm registry
        run: npm config set //registry.npmjs.org/:_authToken $NPM_TOKEN
        env:
          NPM_TOKEN: ${{ secrets.NPM_TOKEN }}
      - uses: ./.github/actions/callisto-action
        with:
          # Publishing is performed by the durable repository workflow after merge.
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
          CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}
          NPM_TOKEN: ${{ secrets.NPM_TOKEN }}
```
