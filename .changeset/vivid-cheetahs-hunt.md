---
callisto-cli: minor
---

`callisto status` shows real pending severity (cascade, fixed/linked groups) and folds in `validate`'s well-formedness checks under `--check`; `callisto validate` is removed.

Breaking:
- `callisto validate` is removed. Use `callisto status --check` and `callisto release --dry-run`.
- `status --check` exit codes: 1 = diagnostic errors (any pending), 2 = no errors, changesets pending, 3 = no errors, nothing pending. 0 stays reserved for `status` without `--check`.
- The moon extension's `validate` dispatch now returns an explicit `E_REMOVED` error instead of falling back to `status`.
