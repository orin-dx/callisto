# REQ-DX-V1: world-class release DX and correctness for v1

Stakeholder: Callisto owner (sole maintainer); users releasing Rust, npm (incl. napi-rs) and Python monorepos.

Status: open. The command surface and release machinery exist; [the roadmap](../projects/ROAD-TO-V1.md) lists the verification and onboarding work that remains. Real local npm, Cargo and PyPI tests exist; the Cargo and PyPI ones drive the publisher directly and do not cover registry binding or observation.

Why: Callisto's release engine is rigorous but its surface is confusing and its setup is heavy. Goal: a new repo goes from `callisto init` to its first published release in under 10 minutes, with one obvious release command and no known correctness bugs.

## Decided by the owner

- The main verb is `release`. Flow: `add` → `version` → `release`.
- `callisto release` publishes every package whose version has no release yet: registry publish, tag, forge release. `--dry-run` previews it on any branch.
- User-facing commands shown in `--help`: init, add, status, version, release, pre, snapshot, completions. Plumbing stays callable but hidden.
- `add` keeps both modes: interactive wizard on a TTY, flags for automation, clear error with the needed flags when neither.
- Breaking changes are fine pre-1.0; no external users.
- Current implementation scope is Rust/Cargo, npm/pnpm (including napi-rs) and Python/PyPI. Keep architecture room for Go and C#/.NET/NuGet without implementing those adapters now.
- Prefer canonical ecosystem-qualified references and deliberate legacy migration over collision-triggered identity magic. Release ownership, native distribution identity, version authority and effects are separate responsibilities; the detailed proposal is [the scoped model](../architecture/polyglot-v1.md).
- Do not build a universal package manager, build scheduler, cross-language inference engine or persisted execution journal. Native tools/CI own compilation, packing and authentication.

## Done when

- `callisto --help` lists exactly the 8 user commands; each has one plain-language line.
- `callisto release --dry-run` with no flags prints the full plan on any branch; `callisto release` with no flags performs it locally with credentials in the environment.
- The CI split (plan → build → execute across jobs) works through `release` flags or hidden subcommands, and callisto's own workflow uses them.
- `publish`, `plan-publish`, `tag`, `filter-plan` are removed; one plan implementation serves preview and execution and includes every check listed above.
- Text output uses colour/tables on a TTY, plain text when piped or `NO_COLOR` is set, no schema versions in text; `status` shows cascaded bumps.
- A verified defect is fixed with regression coverage across affected siblings. CI runs the shipped runtime behavior; no shipped Cargo feature variants. The real local npm/Cargo/PyPI tests fail, not skip, under CI when a tool is missing, and production binding/observation paths are checked separately.
- `callisto init` detects ecosystems/packages, asks only intent (fixed vs independent; release artifacts if binaries exist), writes minimal config, and can write a GitHub workflow using `callisto-action` of <= 40 lines; a non-interactive mode exists.
- Supported fixtures cover separate native packages, and dual Cargo/npm outputs. Native dependency ranges/names, all affected lockfiles and built distribution metadata agree after versioning.
- A retry completes required missing effects without inventing a new version, even after tag creation or a partial Python/platform upload. Required outputs and shared public-announcement prerequisites are explicit; version/tag existence alone is not completion evidence.
- Generated workflows build/place only intended supported outputs and configure native authentication without registry secrets in plan/build jobs. This repo retains no GitHub Environment or separate reviewer gate; the reviewed merge remains the authorization.
- Fresh-repository runs measure init-to-first-publication against the existing under-ten-minute goal, including commit and credential steps. The supported/unsupported boundary is documented; no timing evidence is claimed until measured.

## Decisions still open

Authenticated private Python indexes and HTML-only indexes; npm dist-tag verification/promotion; universal registry checksum requirements; wrapper-specific bump severity; inferred-only/manual release commits under ADR 2; exact canonical-reference migration and release-owner wire representation. Record those choices before their implementation; do not make unstated preferences release gates.

Go major-qualified module paths/tag publication and NuGet normalized versions/RID/TFM assets are model examples, not promised runtime support or additional v1 gates.

## Out of scope

- Non-GitHub forges (later).
- Moon plugin (replaced by a proto plugin separately).
- Crate consolidation / license split.
- Production Go or NuGet adapters, arbitrary SCM-derived Python version backends, automatic foreign-registry dependency inference and cross-registry atomic rollback.
