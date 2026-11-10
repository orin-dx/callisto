---
callisto-cli: patch
---

`callisto release pending --format json` now carries the same envelope as every other JSON report.

- The output gains `schemaVersion`, `command` and `dryRun`; the `pending` list and its fields are unchanged, so anything that reads `.pending[0].source` keeps working.
- A failing `release pending` reports `command: "release pending"` instead of `"release"`, so a script can tell which subcommand failed.

Action: if a script checks for `command == "release"` on a failing `release pending`, match `release pending` instead.
