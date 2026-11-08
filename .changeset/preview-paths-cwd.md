---
callisto-cli: patch
---

`version --emit-preview` and `compose-pr-body --preview` now resolve a relative path against `--cwd`, like the other file flags. Only `--emit-decision` stays relative to the workspace root.
