---
callisto-graph: patch
callisto-cli: patch
---

Four instances of a value being re-derived instead of reused, all fixed the same way (compute once, have both branches consume the same value):

- `crates/callisto-cli/src/commands/status.rs`'s `--check` exit code recomputed `has_changesets` from `report.packages` instead of reading the field `callisto-graph`'s status command already computed and stored on `StatusReport`.
- `crates/callisto-graph/src/commands/validate.rs`'s `--staged` and `--since` branches each ran their own `git diff`, error-mapped it, and filtered changesets identically, differing only in the git args. Factored into one `changesets_touched_by` helper.
- `crates/callisto-graph/src/commands/tag.rs`'s floating-major tag computation (template lookup, version extract, grammar resolve, parse, render, existence check) was written out fully in both the dry-run preview branch and the write branch. Factored into `plan_floating_major`, called by both; the permit still gates only the actual `create_floating_major` call.
- `crates/callisto-cli/src/commands/publish.rs` and `plan_publish.rs` independently wired up the identical `load_workspace` -> `plan_publish` pipeline. Factored into a shared `plan_publish::build_plan` both commands call.
