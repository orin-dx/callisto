---
callisto-cli: minor
---

Every JSON field Callisto writes is now camelCase, so scripts see one naming style.

- `version` and `snapshot` `--format json` reports: `reason.dep_kind` and `reason.dependency_to` are now `depKind` and `dependencyTo`.
- Release plumbing files: the preview's `short_sha` is `shortSha`, and the receipt's `peeled_commit`, `tag_name` and `byte_length` are `peeledCommit`, `tagName` and `byteLength`.
- A test now walks every report and file type and fails on a snake_case key.

Breaking: scripts that read `reason.dep_kind` or `reason.dependency_to` from `version` or `snapshot` JSON must use `depKind` and `dependencyTo`.

Action: keep Callisto and the release action on one version. A release preview or receipt written by 0.19 does not load in this version.
