# Bug-Hunter Findings Ledger

Append-only log of findings from prior bug-hunter runs. Check this before starting a new sweep so you don't re-spend budget rediscovering the same bug. Update an entry's `Fix Status` in place when it changes — don't delete history, mark it.

Format: one entry per confirmed finding. `Fix Status: unfixed | fix applied, untested | fixed (test run + date)`.

---

### [Critical] Linked-group version convergence unimplemented
- **Status**: CONFIRMED
- **Location**: `crates/callisto-graph/src/aggregate.rs:211` (`union_linked`)
- **Classification**: Spec Drift
- **Root Cause**: Spec §G.6.7 requires all named members of a linked group to converge to `max(targets)` before the cascade fixpoint runs. `union_linked` only unions severity, never versions — the convergence step doesn't exist anywhere in the codebase.
- **Failing Scenario**: Two linked packages at `1.4.0` and `2.7.3`, both marked `minor` in one changeset, should both land on `2.8.0`. They instead diverge to `1.5.0` and `2.8.0`.
- **Fix Status**: unfixed (found 2026-07-24/26 session; a prior external report claimed this was "VERIFIED IN TEST SUITE (graph_tests.rs)" — that claim is false, do not trust it)

### [High] Stale dependency-version rewrite on re-cascade
- **Status**: CONFIRMED
- **Location**: `crates/callisto-graph/src/cascade.rs:239`
- **Classification**: Ordering/Staleness
- **Root Cause**: `out.rewrites.entry(key).or_insert(...)` is first-write-wins, but the worklist is processed in lexicographic (`BTreeSet`) order, not topological order. A package that gets re-escalated after its dependents were already rewritten keeps the stale, lower version in those dependents' manifests instead of the correct final one.
- **Failing Scenario**: A depends on B, B depends on C. C is processed first (raises B's severity); B is processed and rewrites A's spec using B's still-low target; B is then re-escalated via C and reprocessed with a higher target, but A's manifest keeps pointing at B's stale pre-escalation version.
- **Fix Status**: unfixed

### [High] `compose-pr-body` silently produces near-empty output
- **Status**: CONFIRMED
- **Location**: `crates/callisto-graph/src/aggregate.rs:112-123` (`_inference` param unused), `crates/callisto-graph/src/commands/version.rs:63-66`, `crates/callisto-graph/src/commands/pr_body.rs:18,26-56`
- **Classification**: Silent Failure
- **Root Cause**: `aggregate()` never invokes conventional-commit severity inference — the parameter is renamed `_inference` and dead. Packages without a hand-authored `.changeset/*.md` entry get `Severity::None`, get filtered out, `plan.bumps` ends up empty, and `pr_body.rs` returns just the bare `"## Release Preview\n\n"` header with no diagnostic. Secondary bug same file: `pr_body.rs:18` takes `_opts: &PrBodyOptions` and never reads it, so `--existing-body`/`--labels` have no effect.
- **Failing Scenario**: Run `callisto compose-pr-body` in a repo relying on conventional-commit inference instead of hand-authored changesets (or after changesets were already consumed) — output is an empty-looking PR body instead of an error or real content.
- **Fix Status**: unfixed

### [Critical] `validate.rs` is a no-op that always reports success
- **Status**: CONFIRMED
- **Location**: `crates/callisto-graph/src/commands/validate.rs:21` (`_loaded` unused), `crates/callisto-graph/src/commands/mod.rs:21` (`escalate()`)
- **Classification**: Silent Failure
- **Root Cause**: Changesets are loaded into `_loaded` and never used. `diagnostics` starts as an empty `Vec` and `escalate()` only upgrades the severity of *existing* diagnostics — it never adds new ones. `callisto validate` unconditionally returns `valid: true` regardless of actual workspace problems.
- **Failing Scenario**: Any malformed changeset, invalid config, or real workspace inconsistency — `validate` still reports clean.
- **Fix Status**: unfixed — highest-priority fix in the ledger; it actively asserts correctness rather than just going silent.

### [High] `plan-publish` computes `is_release` from a proxy signal, not real pending-change state
- **Status**: CONFIRMED
- **Location**: `crates/callisto-graph/src/commands/publish.rs:24-33`
- **Classification**: Ordering/Staleness + Silent Failure
- **Root Cause**: `is_release` is computed by diffing the current manifest version against the last git tag, bypassing the changeset/severity/cascade pipeline entirely.
- **Failing Scenario**: Run `plan-publish` before `callisto version` has bumped manifests (a plausible CI ordering) — every package's manifest version still equals its last tag, so `is_release` is false across the board and `releases`/`rust_crates`/`npm_main_packages` all come back empty even with real pending changesets.
- **Fix Status**: unfixed

### [Critical] `--dry-run` is parsed but never checked before writing files
- **Status**: CONFIRMED
- **Location**: `crates/callisto-cli/src/cli.rs` (`GlobalArgs.dry_run`), `crates/callisto-cli/src/commands/version.rs:14-40`
- **Classification**: Unchecked Flag
- **Root Cause**: `dry_run` is parsed into `GlobalArgs` and never read anywhere else in the crate. `version::handle` calls `apply_version_plan` unconditionally with `ApplyOptions { transient: false }`.
- **Failing Scenario**: `callisto version --dry-run`, as documented in README.md as a safe preview, writes manifests/changelogs for real.
- **Fix Status**: unfixed

### [Medium] Miette diagnostics are dead code
- **Status**: CONFIRMED
- **Location**: `crates/callisto-cli/src/main.rs:30-33`
- **Classification**: Silent Failure / DX
- **Root Cause**: Errors are surfaced via plain `eprintln!("{err}")` (`Display`), not miette's renderer. Every `#[diagnostic(code(...), help(...))]` annotation on `CliError` never reaches the terminal.
- **Fix Status**: unfixed — a prior external report claimed this was "AUDITED & FIXED"; that claim is false, do not trust it.

---

## Known-unverified external claims (re-check before trusting, don't re-report as new if still true)

- "Silent fallback version defaults fixed" — `cascade.rs:278-297`'s `unwrap_or_else(|| Version::semver(1,0,0))` looked still-live as of the 2026-07-26 session; not independently re-confirmed since.
- "Pre-release lifecycle verified in `lifecycle_e2e_tests.rs`" — not independently re-checked.
- "Dependency range/catalog constraint calculation verified" — not independently re-checked.
