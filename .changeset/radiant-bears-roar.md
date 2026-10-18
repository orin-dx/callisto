---
callisto-cli: patch
---

A library that ignores its `Cargo.lock` can release again: `callisto version` refreshes the lockfile but no longer fails trying to stage it. A changelog or manifest that a `.gitignore` ignores now stops `version` before anything is written, naming the file, instead of failing after the edits. Two `callisto add` runs at the same moment can no longer replace each other's changeset.
