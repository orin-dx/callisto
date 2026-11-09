---
callisto-cli: patch
---

`callisto release` and `release-pr` resolve relative file flags against `--cwd`, so `--cwd` works the same for every flag.

- Before, they resolved against the directory you ran from.
- If a script passes relative paths together with `--cwd`, make them relative to that directory or absolute.
