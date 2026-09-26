# 7. Platform packages are owned through optionalDependencies and release with their owner

Status: Accepted (implemented in #111)

## Context

- napi-rs distributions ship one main npm package plus N per-platform packages. The main package names them in `optionalDependencies` at exact versions, so platforms must be on the registry first or installs 404 (docs/00-design.md §0, §9.2). #111 extends the same pattern to esbuild-style distributions and uses `os`/`cpu` in `package.json` as the platform signal.
- The original design derived platforms from `napi.targets`, and had `callisto init` offer to write them into `callisto.toml` as a user-owned `[[fixed-group]]`; steady state would not re-derive, which "avoids permanent coupling to `@napi-rs/cli` internals" (docs/00-design.md §5.3).
- In practice platform directories usually sit outside the npm workspace globs (oxc's `packages/*/npm/*`) and were never discovered, versioned or published (#111).

## Decision

A `package.json` with no co-located admitted package of another ecosystem, with `os`+`cpu`, that exactly one admitted npm package names in `optionalDependencies` is a `ManifestRole::Platform` manifest of that owner, found inside or outside the workspace globs. It is never a package of its own: it takes the owner's version, has no tag, and each platform publish is a prerequisite of the owner's npm publish. The owner's `optionalDependencies` pins follow. A platform with zero or several owners is not guessed. Inside the workspace it stays its own package with a `platform-package-without-owner` warning. Outside the workspace, several owners give the same warning and it is not released; no owner leaves it undiscovered with no diagnostic (`crates/callisto-graph/src/walk.rs`).

## Options considered

- **Require platform packages in a `[[fixed-group]]`** — not required after #111 ("No `[[fixed-group]]` entry needed"). It needed every platform listed by hand and could not see directories outside the workspace globs. Fixed-group platform members remain supported as an optional path (b46079a62: "plus any [[fixed-group]] platform members").
- **Derive platforms from `napi.targets` into explicit config** — the §5.3 design, not chosen in #111. #111 uses `optionalDependencies` as the owner signal "so napi addons and esbuild-style native CLIs behave the same" (b46079a62 notes; see Sources).
- **Treat each platform package as an independent package with its own tag** — rejected since the original design: platform packages "are dependents-in-lockstep with the main package's release, not separate release points" (docs/00-design.md §9.2). #111 kept this ("never its own package, never tagged").

## Consequences

- A changeset naming a platform package directly resolves to unknown; changesets must target the owner package.
- Discovery is npm-only and directory-scoped, so maturin platform packages are not covered (docs/projects/ROAD-TO-V1.md, Design). A platform named by several owners is diagnosed, not attached (`walk.rs`).
- `callisto release` refuses a workspace with attached platforms (`callisto::release_requires_ci_route`, via `ci_release_route`); it must release through `release plan`, `release artifact-manifest` and `release execute` in CI (`crates/callisto-cli/src/error.rs`).

## Enforcement

- `crates/callisto-graph/tests/npm_platform_packages_test.rs`: `platform_packages_are_manifests_of_their_owner_not_packages`, `platform_package_without_owner_stays_a_package_with_a_diagnostic`, `platform_package_named_by_two_owners_is_not_guessed`, `unnamed_platform_outside_the_workspace_stays_undiscovered`, `platform_versions_follow_the_owner_without_a_fixed_group`, `snapshot_versions_attached_platforms_too`.
- Attachment logic: `crates/callisto-graph/src/walk.rs` (`PlatformPackageWithoutOwner`). Code map: [`docs/architecture/identity.md`](../architecture/identity.md).

## Revisit when

- Platform packages need an owner outside npm (maturin wheels).
- A distribution pattern ships platform packages that are not named in the owner's `optionalDependencies`.

## Sources

- PR #111 (b46079a62): commit bodies and PR body; `.claude/semantic-model/core-identity.md` in b46079a62
- docs/00-design.md §0, §5.3, §9.2 (`git show 11038b11b^:docs/00-design.md`)
- docs/projects/ROAD-TO-V1.md, Design section
