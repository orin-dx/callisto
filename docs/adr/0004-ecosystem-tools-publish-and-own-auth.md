# 4. Ecosystem tools publish and own authentication

Status: Accepted (implemented in #143)

## Context

- Callisto releases to crates.io, npm, PyPI and GitHub. Each registry has its own auth rules, retries, rate limits and idempotence.
- The original design argued that `cargo` and `npm`/`pnpm` "already do registry auth, retries, rate-limit handling, and idempotence" (docs/00-design.md §9).

## Decision

Callisto decides what to publish and in what order, then runs the ecosystem tool as a subprocess through `CommandRunner`: `cargo`; `npm`, `pnpm`, `yarn` or `bun`; `python -m build` then `twine`; `gh`. The tool uses whatever credentials it finds and reports its own auth errors. Before each effect callisto observes provider state itself (`cargo info`, `npm view`, `curl` against the PyPI simple index, `gh api`, `git ls-remote`) and retries only those read-only observations (`retry_observation`, `crates/callisto-graph/src/commands/release/provider/policy.rs`). Callisto links no HTTP client crate and runs no credential pre-flight.

## Options considered

- **Native registry clients in callisto** — rejected. The original design argued the tools "already do registry auth, retries, rate-limit handling, and idempotence — reimplementing any of that inside callisto would be redundant and would put callisto in the business of tracking every registry's quirks" (docs/00-design.md §9). §9.5 dropped `octocrab`/`reqwest`/`tokio` from the CLI, which "significantly shrinks the CLI's dependency surface".
- **Credential pre-flight before the first effect** — added in #134, removed in #143: "cargo, npm, twine and gh report their own auth failures, and a rerun adopts every effect that already landed, so a second credential model in Callisto only drifts from the tools'".

## Consequences

- Any auth method a tool supports works without callisto modelling it.
- A missing credential surfaces mid-release as E163 (E164 for `gh`), carrying the tool's redacted stderr. Effects before it have landed; a rerun adopts them (ADR 3).
- Callisto parses tool output to classify results (for example "already published"), so a tool's output change can break classification (`crates/callisto-graph/src/commands/registry_argv.rs`).
- Every release machine needs the tools on `PATH`.
- Today observation timeouts and spawn failures skip the retry loop, and PyPI ignores `Retry-After` (docs/projects/ROAD-TO-V1.md, v1 fix plan §1).

## Enforcement

- None automated. The fixed programs are in `crates/callisto-graph/src/commands/release/provider/policy.rs` (`programs`: git, gh, cargo, npm, curl); the publish programs (pnpm, yarn, bun, `python3` or `python`, twine) are built in `crates/callisto-graph/src/commands/registry_argv.rs`.
- No HTTP client crate (`reqwest`, `ureq`, `octocrab`, `hyper`) appears in `Cargo.lock`; nothing checks this.

## Revisit when

- A target registry has no CLI that can publish non-interactively.
- A tool's auth failure can leave a registry in a state a rerun cannot adopt.

## Sources

- docs/00-design.md §9, §9.5 (`git show 11038b11b^:docs/00-design.md`)
- PR #134 (3d0e2909d): pre-flight added and widened
- PR #143 (bde7305f9) "refactor!: let tools own auth; one build; lighter release and init", sub-commit "refactor(release)!: drop the local credential pre-flight"
