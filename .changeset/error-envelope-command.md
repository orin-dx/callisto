---
callisto-cli: patch
---

With `--format json`, a failing `release plan`, `release execute`, `release artifact-manifest` or `release-pr` subcommand now reports its own `command` in the error, and every JSON error carries `dryRun`, like the success reports.
