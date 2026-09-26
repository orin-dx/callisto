# 11. Packages are keyed by directory; promotion is display-only

Status: Proposed

## Context

- A package's id was its primary manifest name. When a second directory declared the same name in another ecosystem, walking promoted both to `cargo/foo` and `npm/foo` (`crates/callisto-graph/src/walk.rs`).
- Everything derived from the id changed with it. The default tag template was `{id}@{version}`, so an existing Cargo `foo` looked for `cargo/foo@*` tags, lost `foo@1.0.0`, and planned its next release from no prior tag.
- `pre.json` `initialVersions` and selectors already tolerate promotion: keys are display ids with a bare-name fallback, and `cargo/foo` resolves whether or not the package is promoted.

## Decision

- A package's stable identity is its workspace-relative directory: `Package::key() -> Option<PackageKey>` in `crates/callisto-model/src/package.rs`.
- `PackageId` is the display and selector form. Promotion changes only how it prints (`foo` becomes `cargo/foo`) and which bare selectors are ambiguous (E103).
- Nothing that outlives a run derives from the display form. The default tag template is `{name}@{version}` from the native name. The CHANGELOG path comes from the directory. `pre.json` falls back to the bare-name key.
- Two packages that would share a default tag template fail with E101 until all but one of them has a `tag-template`.

## Options considered

- **Primary manifest name as key** — rejected: promotion changed the id, and with it the tag lookup of an already-released package.
- **Re-key every in-run map by `PackageKey`** — rejected for now: about 66 maps over roughly 1,000 `PackageId` uses. Promotion is computed once per walk, so within a run `PackageId` maps one-to-one onto `PackageKey`; re-keying would change no behavior.
- **Keep the display id in the default tag and fall back to `{name}@` tags** — rejected: after promotion both packages match `foo@*`, so the new package would adopt the old one's releases.

## Consequences

- Adding a same-named package in another ecosystem no longer changes the existing package's tags, CHANGELOG or pre-release baseline.
- Workspaces that are already promoted and have no `tag-template` now fail with E101. Their existing `cargo/foo@*` tags stay reachable through `tag-template = "cargo/foo@{version}"` or `previous-tag-templates`.
- A pending changeset that names the bare `foo` is ambiguous after promotion (E103, WS-ID-06) and must be qualified.

## Enforcement

- `TagTemplate::default_for` in `crates/callisto-model/src/tag.rs` and the E101 check in `crates/callisto-graph/src/tags.rs`.
- `promotion_changes_the_display_id_but_not_the_key_or_default_tags` in `crates/callisto-graph/src/walk.rs`; `crates/callisto-cli/tests/identity_promotion_tests.rs`.
- WS-ID-03 and WS-ID-08 in `docs/specs/workspace.json`.

## Revisit when

- Something persists a package reference that is not a selector, tag or path, such as a cache keyed by package. It should use `PackageKey`.
- A package can move directories while keeping its identity.

## Sources

- Owner decision, `docs/projects/ROAD-TO-V1.md` "v1 fix plan" decisions (2026-09-25)
- The fix in this PR: "refactor(identity)!: key packages by directory; promotion is display-only"
