---
callisto-cli: minor
---

Breaking: eight diagnostic codes that callisto no longer emits are gone from the JSON schema.

**What changed**
- `--format json` output never contains `napi-coordination-not-yet-supported`, `pre-major-inference-inert`, `changelog-section-not-found`, `changeset-read-error`, `git-discovery-failed`, `changelog-read-error`, `bare-rule-matches-multiple-ecosystems` or `publish-target-not-implemented`
- The `DiagnosticCode` enum in the schema no longer lists them

**Action**
- Remove any match on these codes from scripts; a schema validator pinned to the old enum should be updated
