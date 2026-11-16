---
callisto-cli: patch
---

`callisto version` no longer crashes on a `pyproject.toml` whose `project` key is not a table.

- Before, a file like `project = ""` made the version write panic with "index not found".
- Now `version` stops with the missing-version error and changes nothing.
- You do not need to do anything. If you hit it, make `project` a `[project]` table with a `version`.
