---
callisto-model: minor
callisto-graph: minor
callisto-cli: minor
callisto-manifests: minor
callisto-conventional: minor
callisto-vcs: minor
callisto-moon: patch
callisto-changelog: patch
callisto-format: patch
---

# Release pipeline correctness, `callisto matrix`, and cross-ecosystem identity fixes

## Breaking Changes

- **`CascadeInput` needs two new fields** — `callisto-graph`
  - Add `tags: &TagIndex` and `identity: &IdentityIndex` to any direct construction.

- **`Manifest::persist` is now required** — `callisto-manifests`
  - `write_version`, `update_dependency_spec`, `update_optional_dependencies` only mutate in memory now.
  - Call `persist()` to write. Custom `Manifest` implementors must add it.

- **JSON report fields renamed** — `callisto-model`, visible via `callisto-cli --format json`
  - `ValidateReport.valid` → `ok`
  - `ComposePrBodyReport.pr_body` → `body`
  - `TagReport.created_tags` → `tags` (plus new `already_existed: bool`)
  - `StatusReport` gained `hasChangesets`, `lastReleasedVersion`, `releaseTrigger`
  - Update anything parsing this JSON, including the shipped GitHub Action.

- **`callisto-conventional` no longer depends on `callisto-vcs`**
  - `fetch_commits`/`infer_severity` now take `&dyn callisto_model::CommitWalker`.
  - `ConventionalError::Vcs` renamed to `ConventionalError::CommitWalk`.

- **`PublishTarget::Npm.restricted: bool` → `access: Option<NpmAccess>`** — `callisto-model`
  - npm's `publishConfig.access` has three states (unset, public, restricted); a bool couldn't hold them.
  - Explicit `"public"` on an unscoped package was previously dropped silently.

- **New `PackageIdParseError::LeadingHyphen` variant** — `callisto-model`
  - Split out from `PathTraversal`, which was wrongly reported for inputs with no `..` at all.

## Features

- **New `callisto matrix [--package <name>]` command**
  - Discovers napi and maturin platform targets from `package.json`/`pyproject.toml`.
  - Builds a per-triple CI table: host runner, cross-compile flag, artifact name.
  - Reports `engines.node`/`requires-python` versions.

- **Platform-manifest and `optionalDependencies` planning actually works now** — `callisto-graph`
  - `plan_version` computes real writes for a bumped owner's platform siblings in a Fixed group.
  - These fields were previously always empty.

- **Fixed groups converge correctly** — `callisto-graph`
  - Siblings in a Fixed group now bump to a shared target instead of drifting independently.

- **Better cross-ecosystem package discovery** — `callisto-graph`
  - Cargo/npm/pnpm workspace membership is honored — fuzz targets and scratch examples stop showing up as release-managed packages.
  - Same-named packages in different ecosystems (a Rust crate and its Python binding, say) resolve correctly instead of erroring.
  - `cargo:foo`/`npm:foo` prefixed group-member names now work.

- **One changelog renderer, everywhere** — `callisto-graph`
  - PR bodies render through the same code CHANGELOG.md uses.
  - Inference-driven and new-group-member bumps get real entries instead of a placeholder.
  - A bump with multiple causes (changeset + cascade, say) lists all of them, not just one.

## Security

- Package names are validated before hitting `cargo publish`/`npm publish`/`pypi publish`, closing an argument-injection hole. (`callisto-graph`)
- `publishConfig.registry` for npm packages is checked against an allowlist, blocking SSRF via a malicious registry URL. (`callisto-graph`)
- The shipped GitHub Action no longer interpolates untrusted input directly into shell steps.
- Credentials are redacted from git/registry-CLI stderr before it lands in error messages. (`callisto-vcs`, `callisto-model`, `callisto-graph`)
- Subprocess output capture is bounded, so a runaway child process can't exhaust memory. (`callisto-cli`)
- Tag names with a leading hyphen are rejected and shelled refs are qualified, closing a git argument-injection hole. (`callisto-vcs`, `callisto-model`)

## Fixes

- **`callisto tag` no longer fabricates a sha**
  - When a tag already exists at a different commit, it resolves the real target via `git rev-parse`.
  - Applies in both apply and `--dry-run` modes.

- **GitHub Action's `status --check` branching fixed**
  - Now matches the CLI's real exit codes: 1 = pending, 2 = none pending.
  - The release-PR step was unreachable before this.

- **`apply_version_plan` is more reliable**
  - Now idempotent, refreshes the npm lockfile, and persists writes correctly.
  - Previously risked a lost write.

- **Manifest writes to the same file are now batched**
  - One open/mutate/persist cycle, processed strictly in order.
  - Closes a stale-handle data-loss bug.

- `plan_publish` reads the real `CHANGELOG.md` section instead of always leaving it blank.
- Merged-branch commits crossing the `since` boundary are no longer dropped from `commits_since_with_pathspec`. (`callisto-vcs`)
- Peer-dependency escalation respects the actual cascading severity instead of always jumping to Major.
- PEP 440 `is_prerelease`/`VersionReq` normalization fixed. (`callisto-model`)
- Cargo caret-range coverage now correctly excludes pre-release versions. (`callisto-format`)
- Several duplicate diagnostic codes — different errors that shared one code — now have their own.
- `GraphError::ConflictingGroupMembership` is actually detected now, when two differently-spelled group-member strings resolve to the same package.
- Smaller fixes: BREAKING CHANGE footer parsing, Python dependency-name normalization (PEP 503), npm package-manager auto-detection, Cargo compound dependency-range rewriting, workspace-inherited-version publish guard.

## Performance

- `status`, `plan_snapshot`, `plan_publish`, and `plan_version` share one `GitAccess` instead of rediscovering the repo per package or command.
- Tag existence checks use the cached `TagIndex` instead of a lookup per release.
- `callisto matrix` parses each platform manifest once and reuses it.

## Housekeeping

- Docs updated to match shipped behavior (design/spec, ARCHITECTURE.md, README.md).
- Dependency cleanup: several deps centralized to `workspace.dependencies`; unused AGPL dev-dependency removed from Layer 1 crates.
