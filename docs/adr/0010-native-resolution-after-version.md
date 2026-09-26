# 10. The workspace resolves natively after `version` or `snapshot`

Status: Proposed

## Context

- 0.7.2 shipped `callisto-cli` requiring `callisto-graph = "0.7.0"`: the spec still covered the released version, so the out-of-range-only cascade left the floor unraised.
- `cargo metadata --locked`, `npm ci`, `pnpm install --frozen-lockfile` and `uv lock --check` fail whenever a manifest and its lockfile disagree, whether the spec is stale or the lockfile is.
- `snapshot` only rewrites specs a patch bump would break, so a minor or major snapshot bump leaves a covering-but-stale spec in place and `cargo metadata` fails after it.

## Decision

After `version` or `snapshot`, the workspace must resolve with each ecosystem's locked install. Two rules produce that:

- A dependent released in the same run has its spec on a bumped internal dependency raised to the new version, keeping operator and precision, whether or not the old spec still covers it. A dependent that is not releasing is left alone unless its spec is out of range.
- Lockfiles are refreshed by default after every apply; `--no-refresh-lockfiles` opts out.

## Options considered

- **Rewrite only out-of-range specs** — rejected: this is what let 0.7.2 publish requiring `callisto-graph = "0.7.0"`; a caret-compatible floor can still be stale relative to what the dependent actually needs from its co-released dependency.
- **Make lockfile refresh opt-in** — rejected: leaves `npm ci` and equivalents failing by default, which is the invariant this ADR exists to close.

## Consequences

- A fixed or linked group releasing together always has every internal spec at the new shared version, not just a covering one.
- Every `version` and `snapshot` run does more work per ecosystem (a lockfile refresh); a failed refresh is a coded error rather than a silent stale lockfile.

## Enforcement

- `crates/callisto-graph/src/cascade.rs`: a releasing dependent's runtime, optional, build and peer specs on internal dependencies are raised regardless of coverage; dev dependents, `workspace = true`, npm `workspace:` and catalog specs are unaffected.
- VER-CAS-01 and VER-CAS-06 in `docs/specs/versioning.json`.

## Revisit when

- A dependent needs to pin below its dependency's newly released version on purpose (e.g. a deliberate compatibility window); today's rule always raises the floor.

## Sources

- Owner decision, `docs/projects/ROAD-TO-V1.md` "v1 fix plan" decisions (2026-09-25)
- The fix in this PR: raising the floor of co-released dependents in `cascade.rs`
