# 4. Ecosystem tools publish and own authentication

Status: Accepted

## Context

- Callisto releases to crates.io, npm, PyPI and GitHub. Each registry has its own auth rules, retries, rate limits and idempotence.
- `cargo`, `npm`/`pnpm`, `twine` and `gh` already handle all of that (docs/00-design.md §9).

## Decision

Callisto decides what to publish and in what order, then runs the ecosystem tool as a subprocess through `CommandRunner` (`cargo`, `npm`/`pnpm`, `twine`, `gh`; observation also uses `curl` for the PyPI simple index). The tool uses whatever credentials it finds and reports its own auth errors. Callisto has no registry HTTP client and no credential pre-flight.

## Options considered

- **Native registry clients in callisto** — rejected in the original design: the tools "already do registry auth, retries, rate-limit handling, and idempotence — reimplementing any of that inside callisto would be redundant and would put callisto in the business of tracking every registry's quirks" (docs/00-design.md §9). §9.5 removed a planned in-process publish pipeline with retry/backoff, registry API queries and `octocrab`/`reqwest`/`tokio`, which "significantly shrinks the CLI's dependency surface and the amount of registry-specific logic that needs testing".
- **Credential pre-flight before the first effect** — added in #134, removed in #143: "cargo, npm, twine and gh report their own auth failures, and a rerun adopts every effect that already landed, so a second credential model in Callisto only drifts from the tools'".

## Consequences

- Any auth method a tool supports (tokens, config files, keyrings, OIDC) works without callisto knowing about it.
- A missing credential surfaces mid-release as the tool's own error. Effects before it have landed; a rerun adopts them (ADR 3).
- Callisto parses tool output to classify results (for example "already published"), so a tool's output change can break classification (`crates/callisto-graph/src/commands/registry_argv.rs`).
- Every release machine needs the tools on `PATH`.

## Enforcement

- None automated. The invoked programs are listed in `crates/callisto-graph/src/commands/release/provider/policy.rs` (`programs`) and `crates/callisto-graph/src/commands/registry_argv.rs`.
- No HTTP client crate (`reqwest`, `ureq`, `octocrab`, `hyper`) appears in any `crates/*/Cargo.toml`; nothing checks this.

## Revisit when

- A target registry has no CLI that can publish non-interactively.
- A tool's auth failure can leave a registry in a state a rerun cannot adopt.

## Sources

- docs/00-design.md §3.1 ("Publishes packages itself: ✗ — deliberately"), §9, §9.4, §9.5 (`git show 11038b11b^:docs/00-design.md`)
- PR #134 (3d0e2909d): pre-flight added and widened
- PR #143 (bde7305f9): "refactor(release)!: drop the local credential pre-flight"
