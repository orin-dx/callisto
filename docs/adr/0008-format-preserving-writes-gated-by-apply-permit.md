# 8. Format-preserving manifest writes gated by ApplyPermit

Status: Accepted

## Context

- Callisto rewrites files users own and review: `Cargo.toml`, `package.json`, `pyproject.toml`, changelogs, `pre.json`.
- Every mutating command has `--dry-run`. "Respect `--dry-run`" was once a convention, and `pre enter`, `pre exit` and `init` forgot it: `pre` wrote `.changeset/pre.json` unconditionally (d15d4c2b0; `crates/callisto-model/src/permit.rs`).
- A partial or torn write leaves a workspace neither before nor after a version.

## Decision

- TOML edits go through `toml_edit`, changing the existing item in place. JSON goes through `serde_json` with `preserve_order`, and indentation, line endings and BOM are detected on read and reproduced on write (`FormatFingerprint`).
- Every disk write goes through `callisto_model::atomic::atomic_write`: temp file in the target's directory, fsync, rename, then fsync the parent and grandparent.
- Write primitives take `&ApplyPermit`. Its only field is private; outside tests the only constructor is `ApplyPermit::granted_unless_dry_run(dry_run)`, which returns `None` on a dry run.

## Options considered

- **Parse into typed structs and serialize back (serde round-trip) for TOML** — rejected: in-place `toml_edit` edits are "what makes `toml_edit` preserve comments, blank lines, key order, and inline-table formatting the caller did not touch" (docs/01-spec.md CM.4.1). JSON has no comments, so an order-preserving round trip plus fingerprint is enough (CM.5.1).
- **Regex or line-based edits** — reason not recorded. The rule appears in AGENTS.md invariant 2 without a stated reason.
- **Convention-based dry-run checks** — rejected: "A convention every new write site must remember will eventually be forgotten at one" (`permit.rs`), after `pre` and `init` did (d15d4c2b0). With a permit, a handler that forgets the check has nothing to pass and fails to compile (2c27f8d0b).

## Consequences

- Release PR diffs show only version and dependency changes.
- CRLF files stay CRLF: `Cargo.toml` was once rewritten to LF on every write until the fingerprint was shared (435c05a17).
- Tests need `ApplyPermit::force_for_tests()`, behind `cfg(test)` or the `callisto-model/test-util` feature, enabled only under `[dev-dependencies]`.
- The permit is a type check only. It does not stop code that calls `std::fs` directly.

## Enforcement

- Type: `ApplyPermit` (`crates/callisto-model/src/permit.rs`), required by `atomic_write` (`crates/callisto-model/src/atomic.rs`), `Manifest::persist`, changelog writes and the release providers.
- Tests: `crates/callisto-cli/tests/dry_run_invariant_tests.rs` using `assert_no_disk_mutation` (`crates/callisto-fixtures/src/dry_run.rs`); format tests in `crates/callisto-manifests/src/cargo.rs` and `npm.rs` (CRLF, BOM, tab indent, comment and decor preservation).

## Revisit when

- A manifest format appears that no format-preserving editor supports.
- Writes must happen outside a command handler that can read the dry-run flag.

## Sources

- Commits 2c27f8d0b and d15d4c2b0 (ApplyPermit introduced; pre and init fixed)
- `crates/callisto-model/src/permit.rs` module docs; `crates/callisto-model/Cargo.toml` `test-util` comment
- docs/00-design.md §7.6 step 3; docs/01-spec.md CM.4.1, CM.5.1 (`git show 11038b11b^:docs/<file>`)
- Commit 435c05a17 (shared format fingerprint, CRLF fix)
