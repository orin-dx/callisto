---
callisto-model: patch
---

Fix `release-pr decide` emitting snake_case field names (`pull_request_number`, `expected_branch`, `replacement_branch`) inside its JSON `action` payload instead of camelCase. `#[serde(rename_all = "camelCase")]` on an internally-tagged enum only renames the variant tag, not fields inside variants; the executor script reads `.action.pullRequestNumber` via `jq`, got `null`, and ran `gh pr view null`, breaking every release run past an existing managed PR.
