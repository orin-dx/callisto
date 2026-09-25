# 7. Platform packages are owned through optionalDependencies and release with their owner

Status: Accepted

## Context

- napi-rs and esbuild-style distributions ship one main npm package plus N per-platform packages (`os`/`cpu` in `package.json`). The main package names them in `optionalDependencies` at exact versions, so platforms must be on the registry first or installs 404 (docs/00-design.md §0, §9.3).
- The original design derived platforms from `napi.targets` and had `callisto init` write them into `callisto.toml` as a user-owned `[[fixed-group]]`, "to avoid permanent coupling to `@napi-rs/cli` internals" (docs/00-design.md §5.3).
- In practice platform directories often sit outside the npm workspace globs (oxc's `packages/*/npm/*`) and were never discovered, versioned or published (#111).

## Decision

An npm `package.json` with `os`+`cpu` that exactly one npm package names in `optionalDependencies` is a `ManifestRole::Platform` manifest of that owner, found inside or outside the workspace globs. It is never a package of its own: it takes the owner's version, has no tag, and each platform publish is a prerequisite of the owner's npm publish. The owner's `optionalDependencies` pins follow. A platform with zero or several owners is not guessed: it stays its own package (or is not released, if outside the workspace) with a `platform-package-without-owner` warning.

## Options considered

- **List platform packages in a `[[fixed-group]]`** — rejected in #111: that is the old path, and it needed every platform listed by hand ("No `[[fixed-group]]` entry needed" is the stated change). Directories outside the workspace globs were invisible to it.
- **Derive platforms from `napi.targets` into explicit config** — rejected in #111. The reason recorded is scope: #111 targets "napi-rs and esbuild-style npm binary distributions" (needed for oxc-react-docgen), and esbuild-style packages have no `napi.targets`. A fuller reason is not recorded.
- **Treat each platform package as an independent package with its own tag** — reason not recorded beyond #111's statement that a platform is "never its own package, never tagged".

## Consequences

- Changesets that name a platform package directly now resolve to unknown (#111, breaking).
- A Cargo + `package.json` directory takes its npm release id from `package.json` (#111).
- Discovery is npm-only and directory-scoped; maturin and several platform packages per owner are not covered (docs/projects/ROAD-TO-V1.md, Design).
- `callisto release` sends workspaces with attached platforms to the CI route (`ci_release_route`).

## Enforcement

- `crates/callisto-graph/tests/npm_platform_packages_test.rs`: `platform_packages_are_manifests_of_their_owner_not_packages`, `platform_package_without_owner_stays_a_package_with_a_diagnostic`, `platform_package_named_by_two_owners_is_not_guessed`, `platform_versions_follow_the_owner_without_a_fixed_group`, `snapshot_versions_attached_platforms_too`.
- Attachment logic: `crates/callisto-graph/src/walk.rs` (`PlatformPackageWithoutOwner`).

## Revisit when

- Platform packages need an owner outside npm (maturin wheels) or several owners.
- A distribution pattern ships platform packages that are not named in the owner's `optionalDependencies`.

## Sources

- PR #111 (b46079a62): commit bodies and PR body
- Commit cd951653a (init detects attached platforms)
- docs/00-design.md §0, §5.2, §5.3, §9.3 (`git show 11038b11b^:docs/00-design.md`)
- docs/projects/ROAD-TO-V1.md, Design section
