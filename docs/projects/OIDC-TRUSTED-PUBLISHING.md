# OIDC trusted publishing

Plan, not behavior: nothing here ships before it is built, and the specs describe only what exists. Target: after v1. Delete this file when the work ships.

## Goal

Publish to npm, crates.io and PyPI with no long-lived registry secret in the repository. The CI job proves its identity with a short-lived GitHub OIDC token, and each registry accepts it for a package that names that repository and workflow as a trusted publisher.

## What each registry needs

| Registry | Trusted publisher is set up | The job must | Token reaches the tool by |
| --- | --- | --- | --- |
| npm | per package on npmjs.com: owner, repository, workflow file, optional environment | have `id-token: write`; run npm 11.5.1 or later (Node 24 ships it) | npm detects the OIDC environment itself and prefers it over a token. No secret |
| crates.io | per crate in its settings, after a first manual publish | have `id-token: write`; run `rust-lang/crates-io-auth-action`, which exchanges the OIDC token for a short-lived crates.io token and revokes it when the job ends | the action's `token` output, passed to `cargo publish` as `CARGO_REGISTRY_TOKEN` |
| PyPI | per project on pypi.org: owner, repository, workflow file, optional environment | have `id-token: write`; mint an upload token through PyPI's OIDC exchange | PyPI documents the exchange as an implementation detail; the stable interfaces are `pypa/gh-action-pypi-publish` and `uv publish`. Twine takes the minted token as `TWINE_PASSWORD` with `TWINE_USERNAME=__token__` |

A mismatch in owner, repository, workflow file or environment fails, and npm reports it as a misleading E404. Self-hosted runners are not supported by npm. crates.io blocks the `pull_request_target` and `workflow_run` triggers.

## Where Callisto stands

- Generated workflows already give `execute` `id-token: write` (for npm provenance), so the permission itself is not new there. Callisto's own release workflow does not: its policy check allows `id-token: write` only on `build-artifact`.
- Callisto passes the environment through to the tool and never strips `ACTIONS_ID_TOKEN_REQUEST_URL` or `ACTIONS_ID_TOKEN_REQUEST_TOKEN`, so npm can see them. ADR 4 keeps authentication with the ecosystem tool, which fits: Callisto does not need to mint tokens.
- The generated action writes a temporary npm config with an `NPM_TOKEN` entry. With no token set that line may stop npm falling back to OIDC. Unverified.
- Twine is the PyPI uploader, and it cannot mint a token.

## Work

1. Decide the shape: a `callisto.toml` switch, or a generated-workflow option, for "this registry publishes by OIDC", so no secret input is expected for it.
2. npm: omit the token entry from the generated npm config when OIDC is chosen, and check the pnpm, yarn and bun publish paths. Only `npm publish` is documented to use OIDC, so each package manager must be tested.
3. crates.io: generate the auth-action step before `execute` and pass its output as `CARGO_REGISTRY_TOKEN`.
4. PyPI: mint the token in a generated step, or use `uv publish` where it is available; keep twine for the token path.
5. Diagnostics: an E163 hint for the npm E404 case that names the trusted-publisher fields to recheck.
6. Policy and docs: the release workflow policy check, `docs/publishing.md`, the init docs and the generated-workflow tests.

## Decisions for the owner

- Callisto's own workflow: allow `id-token: write` on `execute`? With OIDC there is no registry secret at all, which is the intent of "only `execute` gets registry secrets", but the policy check forbids the permission and changing it is the owner's call. A GitHub Environment stays out either way.
- One job holds the OIDC permission while the project's own build code runs (cargo build scripts during `cargo publish` verification, npm lifecycle scripts). That is also true of a long-lived token today, and a short-lived scoped token is strictly less exposed. Keeping the build in a separate job (the current build-artifact split) limits it further.

## Verification

Needs real packages, so it is owner-run. For each registry: configure a scratch package's trusted publisher, run a release from a scratch repository with no token set, and confirm the version appears and the tag and release are created. Include a deliberately wrong workflow name per registry and check the diagnostic. Local registries (Verdaccio, pypiserver, cargo-http-registry) cannot exercise OIDC, so they cover only the no-token configuration paths.

## Sources

- npm: https://docs.npmjs.com/trusted-publishers
- crates.io: https://github.com/rust-lang/crates-io-auth-action and https://blog.rust-lang.org/2025/07/11/crates-io-development-update-2025-07
- PyPI: https://docs.pypi.org/trusted-publishers/using-a-publisher/
