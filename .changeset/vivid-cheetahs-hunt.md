---
callisto-cli: minor
---

`callisto status` shows real pending severity (cascade, fixed/linked groups) and folds in `validate`'s well-formedness checks under `--check`; `callisto validate` is removed.

Breaking:
- `callisto validate` is removed. Use `callisto status --check` and `callisto release --dry-run`.
- `status --check` exits 0/1: a conventional gate on error-level diagnostics only, regardless of pending changesets. Use `status --format json`'s `.pending` (count of packages with a planned bump) to detect pending changesets.
