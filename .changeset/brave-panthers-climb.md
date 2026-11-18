---
callisto-cli: minor
---

**Generated release workflows publish only what a credential-free job built.** Package scripts, build backends and crate builds no longer run in the job that holds your registry tokens.

- The publish job uploads exactly the Python files and npm tarballs a job without credentials built and sealed, and publishes crates with `cargo publish --no-verify` after that job compiled every crate of the release in one `cargo package --locked` call.
- The simple workflow now has `version-pr`, `package` and `execute` jobs, and the build-matrix workflow gains `attest`, `package-cargo` and `pack-npm` jobs, so only `execute` holds secrets and only the attestation job holds an OIDC token without checking out your code.
- npm packages are packed by their workspace's own package manager, so `workspace:` and `catalog:` ranges are rewritten in the published tarball.
- A local `callisto release` still verifies crates and packs in place, with registry credentials withheld from the build.

**Breaking:** npm publishes now run `npm publish <tarball> --ignore-scripts`. The `prepublishOnly`, `publish` and `postpublish` hooks no longer run, and `prepack` and `prepare` run in the pack job, which has no registry credentials. The simple workflow now publishes only when a push adds or changes the release decision file, not whenever an untagged version is merged. Yarn and Bun authentication and registry settings in `.yarnrc.yml` or `bunfig.toml` are not read because `npm` publishes, so the registry comes from `.npmrc` or from `url` on the `npm` entry of `[registries]` in `callisto.toml`. A release whose npm or PyPI publish is not sealed in the artifact manifest now fails with E306 before anything is published.

**Action:** Delete `.github/workflows/callisto-release.yml` and regenerate it with `callisto init --workflow`, then move work that must run at publish time from `prepublishOnly`, `publish` or `postpublish` into `prepack` or a step of your own after `execute`.
