# 11. Packages are keyed by directory; promotion is display-only

Status: Accepted (implemented in #167)

## Context

- A package's id was its primary manifest name. When a second directory declared the same name in another ecosystem, walking promoted both to `cargo/foo` and `npm/foo` (`crates/callisto-graph/src/walk.rs`).
- Everything derived from the id changed with it. The default tag template was `{id}@{version}`, so an existing Cargo `foo` looked for `cargo/foo@*` tags, lost `foo@1.0.0`, and planned its next release from no prior tag.
- Selectors already tolerate promotion: `cargo/foo` resolves whether or not the package is promoted.

## Decision

- A package's stable identity is its workspace-relative directory: `Package::key() -> Option<PackageKey>` in `crates/callisto-model/src/package.rs`.
- `PackageId` is the display and selector form. Promotion changes only how it prints (`foo` becomes `cargo/foo`) and which bare selectors are ambiguous (E103).
- History recorded under a bare name follows the package that owned the name. Default tags are `{display id}@{version}` (`foo@1.0.0`, then `cargo/foo@1.1.0` once promoted), and a promoted package also finds `foo@*` tags whose commit contains its own manifest and no same-named package's. A bare `pre.json` key moves to the one same-named package that existed when `pre.json` was added. The CHANGELOG path comes from the directory.
- No configuration is needed. E101 is raised only when an explicit `tag-template` equals another package's default; E245 only when a bare `pre.json` key has no single owner.

## Options considered

- **Primary manifest name as key** — rejected: promotion changed the id, and with it the tag lookup of an already-released package.
- **Re-key every in-run map by `PackageKey`** — rejected for now: about 66 maps over roughly 1,000 `PackageId` uses. Promotion is computed once per walk, so within a run `PackageId` maps one-to-one onto `PackageKey`; re-keying would change no behavior.
- **Default tags from the native name (`foo@{version}`) for every package** — rejected: promoted `cargo/foo` and `npm/foo` would share tags, so one of them needed a `tag-template` (E101), and already-promoted workspaces would change their tag names.
- **Fall back to `{name}@` tags without an ownership check** — rejected: the newly added package would adopt the old one's releases.

## Consequences

- Adding a same-named package in another ecosystem no longer changes the existing package's tags, CHANGELOG or pre-release baseline.
- Workspaces that are already promoted keep their `cargo/foo@*` tags unchanged.
- Ownership costs one `git rev-parse` per candidate tag of a promoted package.
- A pending changeset that names the bare `foo` is ambiguous after promotion (E103, WS-ID-06) and must be qualified.

## Enforcement

- `TagTemplate::default_for` in `crates/callisto-model/src/tag.rs`; the ownership fallback in `TagIndex::build` (`crates/callisto-graph/src/tags.rs`); `assign_promoted_pre_keys` in `crates/callisto-graph/src/lib.rs`.
- `promotion_changes_the_display_id_but_not_the_key` in `crates/callisto-graph/src/walk.rs`; `crates/callisto-cli/tests/identity_promotion_tests.rs`.
- WS-ID-03 and WS-ID-08 in `docs/specs/workspace.json`.

## Revisit when

- Something persists a package reference that is not a selector, tag or path, such as a cache keyed by package. It should use `PackageKey`.
- A package can move directories while keeping its identity.

## Sources

- Owner decision, `docs/projects/ROAD-TO-V1.md` "v1 fix plan" decisions (2026-09-25)
- #167 ("refactor(identity)!: key packages by directory; promotion is display-only") and its zero-config follow-up
