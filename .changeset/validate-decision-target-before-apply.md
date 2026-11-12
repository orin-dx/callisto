---
callisto-cli: patch
---

`callisto version --emit-decision <path>` now rejects a bad path before it changes anything, instead of failing after the workspace is already half-versioned.

- Before, only the shape of the path was checked up front, and the file was written at the end. A path that was a directory or the workspace root (`.changeset`, `.`, the root), had a trailing slash, or was inside `.git` failed late with `E213` ("Is a directory", exit 1). By then manifests were bumped and staged and changesets deleted, and a rerun failed with `E121`.
- A relative path through an in-workspace symlink that points outside the workspace wrote the file outside the workspace.
- Now one check runs first, and it also covers `release plan --decision` and `release pending --decision`. All rejections exit 2 with nothing written:
  - `E225`: the path resolves outside the workspace.
  - `E226`: the workspace root, an existing directory, a trailing separator, anything under `.git`, or a path that cannot be resolved.
- A new file under directories that do not exist yet still works.
- If a script expected the late `E213` (exit 1) for these paths, expect `E225` or `E226` with exit 2 instead, and nothing is changed.
