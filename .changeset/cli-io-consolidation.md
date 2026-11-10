---
callisto-cli: patch
---

Relative file flags of `release` and `release-pr` now follow `--cwd`.

- Before, they resolved against the directory you ran from.
- If a script passes relative paths together with `--cwd`, make them relative to that directory or absolute.
