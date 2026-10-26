# Publishing: registry authentication

Registry credential setup only. For the release lifecycle itself (local `callisto release`, generated workflows, recovery, execution semantics), see [`releasing.md`](releasing.md). For the `[registries.<name>]` config schema, see [`config.md`](config.md).

Callisto delegates authentication to `cargo`, `npm`/`pnpm`, and `twine`. Configure credentials for those tools in the publishing job.

---

## First publish

A package that has never been published is released from its manifest version with no changeset. Callisto plans it like any other release, and the first publish is the one most likely to fail for reasons outside Callisto:

- **Check the name first.** A name another project already owns is rejected by the registry. If it already holds the version you are releasing under another repository, `callisto release` stops with E322 and names that repository. Search the registry before the first release.
- **Fill in the metadata the registry requires.** crates.io needs `description` and `license` (or `license-file`) in `Cargo.toml`. A first release of a crate without them stops with E323 before anything is published.
- **Use a token that can create the package.** A token scoped to existing packages cannot publish a new one. Trusted publishing (OIDC) is configured per package on the registry and generally needs the package, or a pending publisher, to exist first, so plan on one token-based first publish. Follow each registry's current documentation.
- **Package managers.** An npm package is published with the manager its workspace uses (`npm`, `pnpm`, `yarn` 4 or `bun`), passing `--tag`, `--access` and, where the manager takes one, `--registry`. Yarn takes no `--registry`; it reads `publishConfig.registry` from the package itself.
- **Scoped npm packages** publish with `--access public` by default. A private scoped package sets `publishConfig.access` to `"restricted"` in its `package.json`, which Callisto passes through.
- **Rehearse.** `callisto release --dry-run` shows what would be published and in which order, and works without credentials.

## Pulling a release

Callisto never deletes, yanks or deprecates anything. Pulling a release is a manual registry action, and the version stays used:

| Registry | Soft pull | Delete |
| --- | --- | --- |
| crates.io | `cargo yank` hides it from new resolution; it stays downloadable and existing lockfiles keep working. | Limited to 72 hours after publishing, or a single owner, few downloads and no dependents. |
| npm | `npm deprecate` keeps it installable with a warning. | `npm unpublish` within 72 hours if nothing depends on it; later only with no dependents, under 300 weekly downloads and a single owner. A used version number can never be reused, and removing every version blocks the name for 24 hours. |
| PyPI | Yank (PEP 592) hides it from normal resolution. | Permanent. A filename cannot be reused even after deletion, and deleting a project frees its name for anyone. |

Registries cache their indexes for minutes, so a pulled version can keep appearing briefly. Prefer the soft pull, then release a fixed, higher version through the normal flow. Leave the git tag in place: Callisto treats an existing tag as a release already made. Yanking does not undo a leaked secret; rotate it.

If you remove a version that is still the manifest version, `callisto release` will try to finish it again and stop with an error. Bump the version with a changeset instead.

See the registries' own pages for current rules: [cargo yank](https://doc.rust-lang.org/cargo/commands/cargo-yank.html), [npm unpublish](https://docs.npmjs.com/policies/unpublish) and the [PyPI help](https://pypi.org/help/).

## Where a publish goes

A registry with no `url` in `callisto.toml` publishes wherever the checkout's own configuration says: `.npmrc` for npm and `.cargo/config.toml` for Cargo. Callisto honors that on purpose, so a private registry set there keeps working, and it means those files decide where the registry token is sent. Review changes to them like code. A release pull request cannot carry such a change: a version commit may only touch manifests, lockfiles, changelogs, changesets and the decision file, and any other path fails verification with E165. Set `url` on the registry entry to pin the destination in `callisto.toml`.

---

## npm

Two supported patterns.

### Pattern A: manual `.npmrc` via `NPM_TOKEN`

Set an `NPM_TOKEN` secret. Add this only in the `execute` job, immediately before the publish step:

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

The release action maps the `PYPI_TOKEN` secret to `TWINE_USERNAME` and `TWINE_PASSWORD` itself. A workflow that runs `twine` directly must set both, because the secret name alone does not configure Twine.

### Generated Callisto workflows

Generated release workflows accept `CARGO_REGISTRY_TOKEN`, `NPM_TOKEN`, and `PYPI_TOKEN` as secrets. They supply each token only during publishing. Custom npm registries need their own native configuration.

Native publish hooks, including `prepare` and `prepublishOnly`, run unchanged with their usual publishing environment.

### Current Python support limits

Callisto supports static PEP 621 or Poetry versions with `python -m build` and Twine. Current limits:

- Co-located Cargo/Maturin projects fail discovery with E007, including Cargo-owned dynamic Python versions.
- Without a sealed build, files an earlier attempt uploaded are matched by name, not by content: a rebuild is not always byte-identical. A package whose wheel name depends on the platform uploads another wheel when rerun on a different machine. A file this run uploads must match the digest the index serves for it; an index that serves no digest is accepted, because only sealed files require one. Builds set `SOURCE_DATE_EPOCH` to the release commit time, which makes hatchling, poetry-core, uv_build and pdm-backend wheels, and setuptools wheels, repeat byte for byte; sdists from setuptools and pdm-backend still differ.
- A sealed build is used only by the generated build-matrix workflow, which builds in a `build-python` job without credentials and seals the files in the manifest. The generated execute job installs a pinned `twine` into a venv on its PATH; a hand-written workflow must provide `twine` itself. Sealing is opt-in at execution: a manifest that seals no Python files, or no manifest at all, builds in the executing job and warns. A sealed file the index serves without a sha256 is treated as differing. A workspace whose only release work is Python uses the single-job workflow, which installs pinned `build` and `twine` before the release step, and is not sealed. With a sealed build the run uploads exactly the built files, and a file on the index under a sealed name with different bytes stops the release (E307).
- A release can only be completed for 14 days after its first file; after that, PyPI [refuses new files](https://blog.pypi.org/posts/2026-07-22-releases-now-reject-new-files-after-14-days/) and the release needs a new version.
- A package tagged by an earlier version of callisto with only some of its files uploaded is reported as released when rerun at the same commit. Finish it by uploading the missing files with `twine`.
- A configured `url` is the simple index Callisto reads and, unless `publish-url` names another endpoint, the one Twine uploads to. Private indexes with read authentication or HTML-only responses need explicit support. Twine credentials do not authenticate `curl`.
- Dependencies in `[dependency-groups]`, Poetry groups and the legacy dev-dependency tables are not followed or rewritten when a workspace package is released. Under uv a version pin on a workspace member is ignored, and Poetry normally references siblings by path, so a stale pin there is harmless; a group that pins a registry version of a sibling is not updated.
- `release` refuses snapshot versions (E169), so a snapshot is never published. Python snapshots use a PEP 440 local label (`0.0.0.dev0+<tag>.<sha>`), which public PyPI would reject anyway. See [PEP 440](https://packaging.python.org/en/latest/specifications/version-specifiers/).
