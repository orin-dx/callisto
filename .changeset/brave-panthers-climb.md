---
callisto-cli: minor
---

**Generated release workflows publish only what a credential-free job built.** Package scripts, build backends and crate builds no longer run in the job that holds your registry tokens.

- The publish job uploads exactly the Python files and npm tarballs that a job without credentials built and sealed.
- Crates publish with `cargo publish --no-verify` after a credential-free job compiled every crate of the release, including a Cargo workspace under a directory such as `native/` with no root manifest.
- The simple workflow now has `version-pr`, `package` and `execute` jobs.
- The build-matrix workflow gains `attest`, `package-cargo` and `pack-npm` jobs.
- Only `execute` holds secrets, and only the attestation job holds an OIDC token without checking out your code.
- npm packages are packed by their workspace's own package manager, so `workspace:` and `catalog:` ranges are rewritten in the published tarball.
- A local `callisto release` still verifies crates and packs in place, with registry credentials withheld from the build.

**Breaking:** npm publishes ran the `prepublishOnly`, `publish` and `postpublish` hooks. They now run `npm publish <tarball> --ignore-scripts`, so those hooks no longer run, and `prepack` and `prepare` run in the pack job, which has no registry credentials. Move work that must run at publish time into `prepack` or a step of your own after `execute`.

**Breaking:** the simple workflow published whenever an untagged version was merged. It now publishes only when a push adds or changes the release decision file. Release through a push that adds or changes that file.

**Breaking:** an npm or PyPI publish that is not sealed in the artifact manifest used to proceed. It now fails with E306 before anything is published. Regenerate the workflow so the credential-free job seals it.

**Breaking:** `npm publish <tarball>` does not read a registry set in `.yarnrc.yml` or `bunfig.toml`, and Yarn and Bun token settings are not used either. A yarn workspace whose `.yarnrc.yml` sets `npmPublishRegistry` or `npmRegistryServer` (at the top level or under `npmScopes`), or a bun workspace whose `bunfig.toml` sets a registry, now fails with E168 before anything is published. Move the registry to `publishConfig.registry` in your package's `package.json`, or to `.npmrc`, and keep the publish token in `NPM_TOKEN`.

**Action:** delete `.github/workflows/callisto-release.yml`, regenerate it with `callisto init --workflow`, and close any stray release PR it opened. Yarn and Bun users: move the registry to `publishConfig.registry` or `.npmrc`. Move work from `prepublishOnly`, `publish` or `postpublish` into `prepack` or a step after `execute`.
