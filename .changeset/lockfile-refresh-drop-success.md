---
callisto-cli: minor
---

The `version` JSON report's lockfile refresh result no longer has `success` or `exitCode`, and the `status`, `version`, `snapshot` and `matrix` reports now say `schemaVersion: 3`.

- `success` was always `true`, because a failed refresh stops the command with error E208 instead of appearing in the report. `exitCode` only mirrored `success`.
- Only the `version` report changed shape. The other three reports change just their `schemaVersion`, because they share one version number.

Breaking: scripts that read `success` or `exitCode` from the lockfile refresh result of `version --format json` get nothing back. Check the command's exit code instead. A script that pins `schemaVersion` to 2 for `status`, `snapshot` or `matrix` must accept 3.
