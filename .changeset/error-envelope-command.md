---
callisto-cli: minor
---

With `--format json`, a failing `release plan`, `release execute`, `release artifact-manifest` or `release-pr` subcommand now reports its own `command` in the error, and every JSON error carries `dryRun`, like the success reports. The library crates change their public API with it (`format_error_json` takes the command and dry-run flag, and `GraphError::PreJsonRead` carries the path), so this release is a minor bump.
