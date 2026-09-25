# REQ-DX-V1: world-class release DX and correctness for v1

Stakeholder: Callisto owner (sole maintainer); users releasing Rust, npm (incl. napi-rs) and Python monorepos.

Status: shipped in #126-#150, except a cargo real-registry e2e test (tracked in `docs/projects/ROAD-TO-V1.md`).

Why: Callisto's release engine is rigorous but its surface is confusing and its setup is heavy. Goal: a new repo goes from `callisto init` to its first published release in under 10 minutes, with one obvious release command and no known correctness bugs.

## Decided by the owner
- The main verb is `release`. Flow: `add` → `version` → `release`.
- `callisto release` publishes every package whose version has no release yet: registry publish, tag, forge release. `--dry-run` previews it on any branch.
- User-facing commands shown in `--help`: init, add, status, version, release, pre, snapshot, completions. Plumbing stays callable but hidden.
- `add` keeps both modes: interactive wizard on a TTY, flags for automation, clear error with the needed flags when neither.
- Breaking changes are fine pre-1.0; no external users.

## State before the work (origin/main, 2026-09-22)
- 17 top-level commands. `publish` only prints a preview (`cli/src/commands/publish.rs`). `plan-publish` prints the same preview. `tag` echoes its input (`--floating-major` ignored). `filter-plan` needs a `PublishReport` nothing produces.
- Real publishing is `release plan` + `release execute`: ~9 required flags (`--source-root`, `--orchestration-revision`, `--decision`, `--intent`, `--receipt`, ...), built for a 3-job CI workflow. `release plan` with no args fails on missing `--out`, `--package`/`--from-release-commit`.
- `release-pr decide/verify/commit-plan`, `compose-pr-body`, `release artifact-manifest`, `matrix`, `schema` are plumbing for the action/workflow but appear in `--help`.
- Help text uses internal jargon ("durable release intents", "forge snapshot", "coordinator revision").
- Output is plain `println!`; `comfy-table` and `anstream` are dependencies but unused. Text output shows internal "schema v1". `status` shows `b (pending: none)` when `version` will bump b by cascade.
- `plan_publish` (graph `commands/publish.rs`) has behaviour `release/derive.rs` lacks:
  - `MissingPlatformDependency` check.
  - dev-dependency cycle tolerance: `derive.rs` treats dev edges as prerequisites, so a dev cycle fails `release plan` with "operation DAG contains a cycle".
  - npm registry allowlist (`UntrustedNpmRegistry`); `derive.rs` accepts any https `publishConfig.registry`.
  - default `--access public` for `@scope/` packages; `derive.rs` passes only explicit access.
  - GitHub release notes from the CHANGELOG section; `derive.rs` path uses `--generate-notes`.
  - `--only` selection, `PublishTargetNotImplemented` warning, "version has no matching tag" detection.
- `release-trigger` is never read for behaviour; with the `inference` feature on, `changeset`-trigger packages still get commit-inferred bumps. The shipped binary builds default features (no `inference`).
- CI tests/coverage run `--all-features`; only `durable_release_e2e_tests` exercises the shipped default-feature build.
- Attestation `source_commit` is the orchestration SHA, not the release-source SHA, on recovery dispatch.
- `.github/actions/callisto-validate` runs `plan-publish ... || true`, hiding failures.
- `init` writes default-valued keys and `[init] ecosystems` bookkeeping; asks nothing; generates no CI.
- A user repo's release workflow is ~350-500 lines (callisto's own, oxc-react-docgen's).
- Publish tests use fakes; no test publishes to a real local registry.

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
