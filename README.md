<p align="center">
  <img src="assets/callisto-logo.png" width="140" alt="Callisto logo" />
</p>

<p align="center">
  <b>Changesets-style version and release manager for Rust workspaces.</b>
</p>

<p align="center">
  <a href="https://github.com/orin-dx/callisto/actions/workflows/callisto-ci.yml"><img src="https://github.com/orin-dx/callisto/actions/workflows/callisto-ci.yml/badge.svg" alt="CI" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT%20%2F%20FSL--1.1--MIT-blue.svg" alt="License" /></a>
</p>

---

Callisto tracks pending changes per package with changeset files, computes version bumps across a dependency graph (with cascade rules for dependents), rewrites manifests in place preserving formatting, and releases each package once its version is unreleased: registry publish, git tag, GitHub release.

Manifest edits go through a concrete-syntax-tree editor (`toml_edit`, fingerprinted `serde_json`) rather than regex, so comments, key order, and whitespace survive. Disk writes are atomic (`NamedTempFile` + `fs::rename`). All Git access runs through the system `git` binary. `unsafe_code = "forbid"` workspace-wide.

## Install

**proto** ([moonrepo/proto](https://moonrepo.dev/proto)), from a release binary:

```toml
# .prototools
[plugins.tools]
callisto = "https://raw.githubusercontent.com/orin-dx/callisto/main/proto/callisto.toml"
```

```bash
proto install callisto
```

**cargo**, built from source (package `callisto-cli`, binary `callisto`):

```bash
cargo install callisto-cli
```

**Release binaries**, from [GitHub Releases](https://github.com/orin-dx/callisto/releases) (tag `callisto@<version>`):

```bash
curl -sL "https://github.com/orin-dx/callisto/releases/download/callisto@<version>/callisto-<target>.tar.gz" | tar -xz
```

Available targets: `aarch64-apple-darwin`, `x86_64-unknown-linux-gnu`, `x86_64-unknown-linux-musl`.

## Quick start

```bash
# 1. Scaffold callisto.toml and .changeset/ in the workspace root
callisto init --yes

# 2. Record a pending change against a package
callisto add --package my-crate:minor --summary "Add authentication middleware"

# 3. See pending changesets and the version bumps they imply
callisto status

# 4. Consume changesets, bump manifests, update changelogs
callisto version

# 5. Publish every package whose current version isn't released yet
callisto release
```

Every subcommand accepts `--dry-run` (preview, no writes) `--format json` (machine-readable output) and `--cwd <path>` (run outside the current directory).

`callisto status --check` exits `1` if there are error-level diagnostics, `0` otherwise (pending changesets alone never fail it) — use it as a CI gate. `callisto pre enter <tag>` / `callisto pre exit` manage prerelease mode. `callisto snapshot --tag <tag>` applies a one-off, non-persistent version bump. `callisto completions <shell>` prints a shell completion script.

## Configuration

Workspace behavior — fixed/linked groups, cascade rules, release targets, per-package overrides — is declared in `callisto.toml`. Full key reference: [`docs/config.md`](docs/config.md).

## Releasing and publishing

`callisto release` handles the common case directly. Workspaces with binary release artifacts (napi/maturin platform packages, or a CLI product like this one) release through a generated CI workflow instead: version PR merges, then CI plans, builds, and executes the release with attested artifacts. See [`docs/releasing.md`](docs/releasing.md) for the lifecycle and recovery, and [`docs/publishing.md`](docs/publishing.md) for registry authentication.

## Errors

Every user-facing error carries a stable diagnostic code and a fix. Full list: [`docs/errors.md`](docs/errors.md).

## More

- [`ARCHITECTURE.md`](ARCHITECTURE.md) — crate map, data flow, invariants.
- [`CONTRIBUTING.md`](CONTRIBUTING.md) — setup, `just` recipes, PR checklist.
- [`docs/specs/`](docs/specs/) — current behavior specs.
- [`SECURITY.md`](SECURITY.md) — reporting a vulnerability.

## License

Mixed: `callisto-model`, `callisto-format`, and `callisto-vcs` are MIT; the rest of the workspace is [FSL-1.1-MIT](LICENSE) (becomes MIT two years after each version's release). See [`AGENTS.md`](AGENTS.md) for the full per-crate table.
