# Publishing: registry authentication

Registry credential setup only. For the release lifecycle itself (local `callisto release`, generated workflows, recovery, execution semantics), see [`releasing.md`](releasing.md). For the `[registries.<name>]` config schema, see [`config.md`](config.md).

Callisto never reads registry credentials directly — it delegates publishing to each ecosystem's own CLI (`cargo`, `npm`/`pnpm`, `twine`), so whatever auth that CLI can see when it runs is what gets used.

---

## npm

Two supported patterns.

### Pattern A: manual `.npmrc` via `NPM_TOKEN`

Set an `NPM_TOKEN` secret. Add this only in the protected `execute` job, immediately before the publish step:

```yaml
- name: Authenticate with npm registry
  run: npm config set //registry.npmjs.org/:_authToken $NPM_TOKEN
  env:
    NPM_TOKEN: ${{ secrets.NPM_TOKEN }}

- run: callisto release execute --intent "$RUNNER_TEMP/release-intent/release-intent.json" --receipt "$RUNNER_TEMP/release-receipt.json" --orchestration-revision "$GITHUB_SHA"
```

`npm config set` writes to the user-level `.npmrc`, which npm reads for every subsequent publish call in the same job. The version-PR action never receives this token — it only creates or updates a reviewed PR.

### Pattern B: `actions/setup-node` with `registry-url`

`actions/setup-node`, given `registry-url`, writes a project-level `.npmrc` that reads the token from `NODE_AUTH_TOKEN`:

```yaml
- uses: actions/setup-node@v4
  with:
    node-version: '20'
    registry-url: 'https://registry.npmjs.org'

- run: callisto release execute --intent "$RUNNER_TEMP/release-intent/release-intent.json" --receipt "$RUNNER_TEMP/release-receipt.json" --orchestration-revision "$GITHUB_SHA"
  env:
    NODE_AUTH_TOKEN: ${{ secrets.NPM_TOKEN }}
```

`NODE_AUTH_TOKEN` is silently ignored by npm unless an `.npmrc` contains a `${NODE_AUTH_TOKEN}` interpolation — that line only exists if `setup-node` ran with `registry-url` set. Setting the env var without that step fails with an auth error, not a variable-not-found error.

### Choosing a pattern

| Criterion | Pattern A (`NPM_TOKEN` + manual step) | Pattern B (`setup-node` + `NODE_AUTH_TOKEN`) |
| --- | --- | --- |
| Depends on `actions/setup-node` | No | Yes — `registry-url` must be set |
| Node.js version management | Separate step or pre-installed | Handled by `setup-node` |
| Recommended when | `execute` job doesn't otherwise need Node setup | `execute` job already uses `setup-node` |
| Release-PR action receives the token | Never | Never |

### Provenance

Give the `execute` job `permissions: id-token: write` and set `NPM_CONFIG_PROVENANCE: "true"` in its env. npm then attests each package to the workflow run and commit; Callisto needs no flag.

---

## Cargo (crates.io)

Set `CARGO_REGISTRY_TOKEN` as a repository secret and pass it to the job environment:

```yaml
env:
  CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}
```

No `.cargo/credentials` setup step needed — cargo reads the environment variable natively.

---

## Python (PyPI)

Publishing uses `twine upload`, which reads `TWINE_USERNAME` (`__token__` for a PyPI API token) and `TWINE_PASSWORD` (the token value):

```yaml
env:
  TWINE_USERNAME: __token__
  TWINE_PASSWORD: ${{ secrets.PYPI_TOKEN }}
```

Do not use `TWINE_API_TOKEN` — twine does not recognize that variable name.

---

## Installer verification (consuming a released binary)

`setup-callisto` verifies a downloaded prebuilt asset with `gh attestation verify` (repository `orin-dx/callisto`, signer workflow `.github/workflows/callisto-release.yml`, using the job's `github.token`) before extracting or using it, and extracts only the `callisto` binary from the archive. `verification` input:

- `require`: verification failure or a missing `gh` aborts the step; the asset is never run.
- `fallback` (default): on failure or missing `gh`, warn, delete the asset, and install from crates.io instead (version pinned to the requested tag; unpinned only for `latest`). The unverified asset is never run.
- `skip`: no verification; warn and use the asset.

`fallback` is the default because releases built before the first attested release carry no attestation, so `require` would hard-fail existing users; a tampered asset is never run in `require` or `fallback`.
