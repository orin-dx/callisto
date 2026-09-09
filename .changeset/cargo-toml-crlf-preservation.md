---
callisto-manifests: patch
---

**Fix: `Cargo.toml` with CRLF line endings was silently rewritten to LF on any write**

`npm.rs`'s `PackageJson` and `python.rs`'s `PyprojectToml` each independently detected and reapplied a file's original line-ending style (`LineEnding::{Lf,CrLf}`, `content.contains("\r\n")` on open, `.replace("\r\n", "\n").replace('\n', "\r\n")` on persist) alongside UTF-8 BOM handling -- but `cargo.rs`'s `CargoToml` only ever tracked `has_bom`, with zero line-ending handling. Any `Cargo.toml` with CRLF line endings (common on Windows / repos with `core.autocrlf=true`) got silently rewritten to LF on the next `write_version`/`update_dependency_spec` + `persist`, unlike the equivalent `package.json`/`pyproject.toml` in the same repo.

Added a shared `common::FormatFingerprint { has_bom, line_ending }` with `detect(content)` and `apply(rendered)`, migrated `npm.rs` and `python.rs` onto it (pure refactor -- their existing BOM/CRLF tests pass unchanged, plus a new direct CRLF regression test for `npm.rs`, which previously had none), and added the same fingerprinting to `CargoToml::open`/`persist` -- the actual fix. Added a CRLF `Cargo.toml` corpus fixture (`callisto_fixtures::corpus::cargo_toml_crlf_no_bom_sample`) and a regression test proving a CRLF `Cargo.toml`'s line endings survive a `write_version` + `persist` round trip.
