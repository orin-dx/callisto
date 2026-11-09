---
callisto-cli: patch
---

`callisto release` and `release-pr` now resolve relative file flags against `--cwd`, not the directory you ran from, so `--cwd` behaves the same for every flag. If a script passed relative paths together with `--cwd`, point them at the `--cwd` directory.
