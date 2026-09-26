# 8. Format-preserving manifest writes gated by ApplyPermit

Status: Accepted (implemented in 2c27f8d0b)

## Context

- Callisto rewrites files users own and review: `Cargo.toml`, `package.json`, `pyproject.toml`, changelogs, `pre.json`.
- `--dry-run` is a global flag; `release plan`, `release artifact-manifest` and `release execute` reject it (`crates/callisto-cli/src/error.rs`). "Respect `--dry-run`" was once a convention, and `pre enter`, `pre exit` and `init` forgot it: `pre` wrote `.changeset/pre.json` unconditionally (d15d4c2b0; `crates/callisto-model/src/permit.rs`).
- An interrupted run must never leave a partially written file (VER-APPLY-05, `docs/specs/versioning.json`).

## Decision

- TOML edits go through `toml_edit`, changing the existing item in place. `package.json` goes through `serde_json` with `preserve_order`, and indentation, line endings and BOM are detected on read and reproduced on write (`FormatFingerprint`). Callisto-owned JSON such as `pre.json` is regenerated canonically (`crates/callisto-model/src/format/pre.rs`).
- Every file content write goes through `callisto_model::atomic::atomic_write`: temp file in the target's directory, fsync, rename, then fsync the parent and grandparent. Deletions (consumed changesets, `pre.json`) and directory creation use `std::fs` directly.
- Write primitives take `&ApplyPermit`. Its only field is private; outside tests the only constructor is `ApplyPermit::granted_unless_dry_run(dry_run)`, which returns `None` on a dry run.

## Options considered

- **Parse into typed structs and serialize back (serde round-trip) for TOML** — reason not recorded. CM.4.1 only says editing the existing item in place, rather than replacing it, "is what makes `toml_edit` preserve comments, blank lines, key order, and inline-table formatting the caller did not touch" (docs/01-spec.md CM.4.1). JSON has no comments, so an order-preserving round trip plus fingerprint is enough (CM.5.1).
- **Regex or line-based edits** — reason not recorded. The rule appears in AGENTS.md invariant 2 without a stated reason.
- **Convention-based dry-run checks** — rejected: "A convention every new write site must remember will eventually be forgotten at one" (`permit.rs`), after `pre` and `init` did (d15d4c2b0). With a permit, a write site has nothing to pass unless its handler minted one (2c27f8d0b).

## Consequences

- TOML manifest diffs in release PRs show only version and dependency-spec changes. `package.json` is re-serialized with serde_json's pretty printer, keeping key order, indentation, line endings, BOM and trailing newline; other layout (for example an inline array) is normalized (`crates/callisto-manifests/src/npm.rs`).
- CRLF manifests and changelogs stay CRLF.
- A failed run can leave earlier files updated; `atomic_write` only prevents a torn single file (VER-APPLY-05).
- Tests need `ApplyPermit::force_for_tests()`, behind `cfg(test)` or the `callisto-model/test-util` feature, enabled only under `[dev-dependencies]`.
- The permit is a type check only. It does not stop code that calls `std::fs` directly, and a handler can still mint one with a literal `false`: `release`, `release artifact-manifest` and `init` do so behind their own dry-run branch (`crates/callisto-cli/src/commands/release.rs`, `init.rs`).

## Enforcement

- Type: `ApplyPermit` (`crates/callisto-model/src/permit.rs`), required by `atomic_write` (`crates/callisto-model/src/atomic.rs`), `Manifest::persist`, changelog writes and the release providers.
- Tests: `crates/callisto-cli/tests/dry_run_invariant_tests.rs` using `assert_no_disk_mutation` (`crates/callisto-fixtures/src/dry_run.rs`); format tests in `crates/callisto-manifests/src/cargo.rs` and `npm.rs` (CRLF, BOM, tab indent, comment and decor preservation).

## Revisit when

- A manifest format appears that no format-preserving editor supports.
- Writes must happen outside a command handler that can read the dry-run flag.

## Sources

- Commits 2c27f8d0b and d15d4c2b0 (ApplyPermit introduced; pre and init fixed)
- `crates/callisto-model/src/permit.rs` module docs; `crates/callisto-model/Cargo.toml` `test-util` comment
- `docs/specs/versioning.json` VER-APPLY-05
- docs/00-design.md §7.6 step 3; docs/01-spec.md CM.4.1, CM.5.1 (`git show 11038b11b^:docs/<file>`)
- Commit 435c05a17 (shared format fingerprint, CRLF fix)
