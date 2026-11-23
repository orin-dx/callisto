---
callisto-cli: patch
---

**`init` and `release --dry-run` say less and say what to do next**

`init` prints each note once and a one-line summary of the generated workflow instead of its whole text, and `release --dry-run` now points the way when it cannot show what you expect.

- `init` no longer repeats its notes under Diagnostics, and shows the workflow path instead of the file contents.
- `release --dry-run` with pending changesets says to run `callisto version` (or merge the release PR) first.
- `release --dry-run` treats a local-path `origin` like no `origin`: it previews with one note that a real release needs a hosted remote.
  - A real `callisto release` still refuses a local-path `origin`.
