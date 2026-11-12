---
callisto-cli: minor
---

Breaking: `version --emit-decision` and `release pending --decision` now reject a decision path outside the workspace, as `release plan` always did.

- `version --emit-decision ../x` or an absolute path outside the workspace used to write the decision file there and exit 0. It now exits 2 with E225 (outside the workspace) or E226 (invalid path) and writes nothing.
- `release pending --decision ../x` used to fail with exit 1 and a raw git "outside repository" error. It now exits 2 with E225 or E226.
- `version --emit-preview` is unchanged, so a preview can still go to a temp file outside the workspace.
- A workspace-relative path, such as the release PR action's `--emit-decision "$INPUT_DECISION_PATH"`, works as before.

Action: a script that passes an absolute or `../` decision path to `version --emit-decision` or `release pending --decision` must use a path inside the workspace.
