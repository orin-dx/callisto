---
callisto-cli: minor
---

**Release trust allows gitignored build output**

A release no longer aborts because a gitignored path exists (built `.node` files, `dist/`, `node_modules/`). Modified tracked files and untracked files still block it, and the error names the path.
