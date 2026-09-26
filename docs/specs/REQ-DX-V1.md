# REQ-DX-V1: world-class release DX and correctness for v1

Stakeholder: Callisto owner (sole maintainer); users releasing Rust, npm (incl. napi-rs) and Python monorepos.

Status: mostly shipped; the remaining gap (a cargo real-registry e2e test) is tracked in `docs/projects/ROAD-TO-V1.md`.

Why: Callisto's release engine is rigorous but its surface is confusing and its setup is heavy. Goal: a new repo goes from `callisto init` to its first published release in under 10 minutes, with one obvious release command and no known correctness bugs.

## Decided by the owner
- The main verb is `release`. Flow: `add` → `version` → `release`.
- `callisto release` publishes every package whose version has no release yet: registry publish, tag, forge release. `--dry-run` previews it on any branch.
- User-facing commands shown in `--help`: init, add, status, version, release, pre, snapshot, completions. Plumbing stays callable but hidden.
- `add` keeps both modes: interactive wizard on a TTY, flags for automation, clear error with the needed flags when neither.
- Breaking changes are fine pre-1.0; no external users.

## Done when
- `callisto --help` lists exactly the 8 user commands; each has one plain-language line.
- `callisto release --dry-run` with no flags prints the full plan on any branch; `callisto release` with no flags performs it locally with credentials in the environment.
- The CI split (plan → build → execute across jobs) works through `release` flags or hidden subcommands, and callisto's own workflow uses them.
- `publish`, `plan-publish`, `tag`, `filter-plan` are removed; one plan implementation serves preview and execution and includes every check listed above.
- Text output uses colour/tables on a TTY, plain text when piped or `NO_COLOR` is set, no schema versions in text; `status` shows cascaded bumps.
- Every bug above is fixed with a test; CI runs the test suite on the shipped feature set; at least one e2e test publishes to real local registries (npm, cargo, PyPI).
- `callisto init` detects ecosystems/packages, asks only intent (fixed vs independent; release artifacts if binaries exist), writes minimal config, and can write a GitHub workflow using `callisto-action` of <= 40 lines; a non-interactive mode exists.

## Out of scope
- Non-GitHub forges (later).
- Moon plugin (replaced by a proto plugin separately).
- Crate consolidation / license split.
