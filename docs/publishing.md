# Publishing: registry authentication

Registry credential setup only. For the release lifecycle itself (local `callisto release`, generated workflows, recovery, execution semantics), see [`releasing.md`](releasing.md). For the `[registries.<name>]` config schema, see [`config.md`](config.md).

Callisto delegates authentication to `cargo`, `npm`/`pnpm`, and `twine`. Configure credentials for those tools in the publishing job.

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

`npm config set` writes the user-level `.npmrc` for later publish calls in the job. The version-PR action receives no token.

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

`NODE_AUTH_TOKEN` needs an `.npmrc` entry containing `${NODE_AUTH_TOKEN}`. `setup-node` writes it only when `registry-url` is set. The environment variable alone does not authenticate npm.

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

Cargo reads this variable directly; no `.cargo/credentials` step is needed. In this repo, inject it only into `execute`.

---

## Python (PyPI)

Publishing uses `twine upload`, which reads `TWINE_USERNAME` (`__token__` for a PyPI API token) and `TWINE_PASSWORD` (the token value):

```yaml
env:
  TWINE_USERNAME: __token__
  TWINE_PASSWORD: ${{ secrets.PYPI_TOKEN }}
```

Do not use `TWINE_API_TOKEN` — twine does not recognize that variable name.

Map the generated workflow's `PYPI_TOKEN` secret to `TWINE_PASSWORD` in the publishing job. The secret name alone does not configure Twine.

### Generated Callisto workflows

Generated release workflows accept `CARGO_REGISTRY_TOKEN`, `NPM_TOKEN`, and `PYPI_TOKEN` as secrets. They supply each token only during publishing. Custom npm registries need their own native configuration.

Native publish hooks, including `prepare` and `prepublishOnly`, run unchanged with their usual publishing environment.

### Current Python support limits

Callisto supports static PEP 621 or Poetry versions with `python -m build` and Twine. Current limits:

- Co-located Cargo/Maturin projects fail discovery with E007, including Cargo-owned dynamic Python versions.
- Files an earlier attempt uploaded are matched by name, not by content: a rebuild is not always byte-identical, and the file set is not sealed between jobs. A file this run uploads must carry the digest the index serves for it.
- A release can only be completed for 14 days after its first file; after that, PyPI [refuses new files](https://blog.pypi.org/posts/2026-07-22-releases-now-reject-new-files-after-14-days/) and the release needs a new version.
- A package tagged by an earlier version of callisto with only some of its files uploaded is reported as released when rerun at the same commit. Finish it by uploading the missing files with `twine`.
- One configured URL serves both simple-index reads and Twine uploads. Private indexes with separate routes, read authentication, or HTML-only responses need explicit support. Twine credentials do not authenticate `curl`.
- Dependencies in `[dependency-groups]`, Poetry groups and the legacy dev-dependency tables are not followed or rewritten when a workspace package is released. Under uv a version pin on a workspace member is ignored, and Poetry normally references siblings by path, so a stale pin there is harmless; a group that pins a registry version of a sibling is not updated.
- Snapshots use a PEP 440 local label (`0.0.0.dev0+<tag>.<sha>`), which public PyPI rejects. See [PEP 440](https://packaging.python.org/en/latest/specifications/version-specifiers/).
