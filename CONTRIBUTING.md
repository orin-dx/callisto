<p align="center">
  <img src="assets/callisto-logo.png" width="140" alt="Callisto Release Engine Logo" />
</p>

<h1 align="center">Contributing to Callisto</h1>

By participating in this project, you agree to abide by the [Code of Conduct](./CODE_OF_CONDUCT.md). Found a security issue? See [SECURITY.md](./SECURITY.md) instead of opening a public issue.

Agent-specific rules (invariants, fixing-bugs workflow, specs/plans, crate/license table) live in [AGENTS.md](./AGENTS.md) — this file covers human setup and PR process and does not duplicate them.

---

## Setup

- Rust toolchain: pinned in `rust-toolchain.toml`, installed via `rustup`.
- [`just`](https://github.com/casey/just): command runner for all workspace recipes.
- [`moon`](https://moonrepo.dev): a few recipes (`build`, `fmt`, `fmt-check`, `lint-affected`) delegate to it; installed like any other tool via [proto](https://moonrepo.dev/proto) (`proto/callisto.toml` shows the same pattern for installing `callisto` itself).

```bash
git clone https://github.com/orin-dx/callisto.git
cd callisto
just hooks   # installs .git/hooks/pre-commit and pre-push
just ci      # full local verification pipeline
```

---

## `just` recipes

| Command | Runs |
| :--- | :--- |
| `just ci` | Everything CI runs except Docker-based actionlint and the binary-dependent release-PR/artifact-preflight jobs: `fmt-check lint test audit doc-check zizmor workflow-contracts release-workflow-behavior coverage 90` |
| `just build` | Debug workspace binaries, via `moon run :build` |
| `just build-release` | `cargo build --release -p callisto-cli` |
| `just test` | `cargo nextest run --workspace` + `cargo test --doc --workspace`; this is what CI runs |
| `just test-moon` | Compatibility path via `moon run :test` for contributors without `cargo-nextest`; never used by CI |
| `just lint` | `cargo clippy --workspace --all-targets -- -D warnings` |
| `just lint-affected` | Clippy scoped to crates affected since the base branch (via `moon query projects --affected`) |
| `just fmt-check` / `just fmt` | Formatting check / apply, via `moon run :format-check` / `:format` |
| `just audit` | `cargo deny check` (advisories, bans, licenses, sources) |
| `just doc-check` | `cargo doc --no-deps --workspace` with warnings as errors |
| `just coverage [threshold]` | `cargo llvm-cov` workspace report to `lcov.info`; `just coverage 90` is what CI's `coverage` job and `just ci` both call, so a coverage failure always reproduces locally |
| `just coverage-per-crate [threshold]` | Same profile data, gated per crate instead of workspace-total (CI runs this non-blocking today; default threshold 90) |
| `just zizmor` | Static security audit of workflow/action YAML (requires `zizmor`) |
| `just workflow-contracts` | Release-workflow contract/policy tests, action pin verification, installer tests — what the CI `workflow-contracts` job runs besides actionlint |
| `just release-workflow-behavior` | Release lifecycle exercised at the real CLI boundary with faked registry/Git/forge/attestation providers |
| `just pre-commit` | `fmt-check` only — fast, runs on every local commit via the installed hook |
| `just pre-push` | `fmt-check` + `lint-affected` — runs on every local push via the installed hook |
| `just hooks` | Installs the native `pre-commit`/`pre-push` git hooks above |
| `just clean` | Clears moon task caches, `cargo clean`, removes `lcov.info` / `callisto-schema.json` |

Maintainer-only recipes not part of the normal PR loop: `mutants` (mutation testing), `machete` (unused deps), `check-api` (SemVer diff via `cargo-semver-checks`), `fuzz`, `provider-fixtures`/`provider-contract` (re-capture/verify the real-provider test fixtures under `testing/fixtures/`).

For iterating on one crate instead of the whole workspace, see AGENTS.md's scoped-iteration guidance (`cargo test -p <crate>`, one invocation at a time — never parallel cargo builds).

---

## Changesets

Non-interactive, for scripts, agents, and most contributors:

```bash
callisto add --package <crate>:<bump> --summary "..."
```

`--package` takes repeatable `name:severity` pairs (`none`, `patch`, `minor`, `major`).

Interactive wizard (omit `--package`/`--summary`, run from a TTY): prompts for packages, then major-bump packages, then minor-bump packages (rest default to patch), then a summary, then a confirmation preview before writing `.changeset/<slug>.md`.

```bash
cargo run --bin callisto -- add
```

---

## PR checklist

CI (`.github/workflows/callisto-ci.yml`) gates on:

- **`fmt`** — `just fmt-check`
- **`clippy`** — `just lint` + `just doc-check`
- **`test`** — `just test`, `just release-workflow-behavior`, and the Release-PR action's binary contract test, on Linux and macOS
- **`release-artifact-preflight`** — builds the release binary for each configured target (macOS arm64, Linux gnu/musl)
- **`coverage`** — `just coverage 90` (workspace line coverage must not regress below 90%); per-crate coverage runs non-blocking
- **`security`** — `just audit`
- **`validate`** — changeset and workspace-status validation
- **`workflow-contracts`** — actionlint, zizmor, action pin verification, and the release-workflow contract/policy tests

Before opening a PR: run `just ci` locally (covers everything above except the Docker-based actionlint step and the binary-dependent artifact-preflight build) and make sure `just pre-push` is clean. Add a changeset for any user-facing change (see above). Follow [Conventional Commits](https://www.conventionalcommits.org/) for commit messages.

## Merging to main

The `main` ruleset requires one approving review, signed commits, squash merges and every CI job passing. Merging a release PR publishes it; there is no second approval. The owner's ruleset bypass is for emergency recovery only.
