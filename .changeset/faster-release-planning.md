---
callisto-cli: patch
---

`callisto release` and the release PR are much faster in very large monorepos.

- With about 1,500 of 1,800 packages releasing at once, `release --dry-run` starts 14 git processes instead of about 3,000: 8.1 s to 1.3 s in a release build, and 7.9 s to 1.6 s on a 20,000-commit history.
- `compose-pr-body --publish-plan` no longer slows down quadratically as more packages release at once: 12 s to 1.5 s in a debug build at 1,800 packages.
- The output is unchanged.
