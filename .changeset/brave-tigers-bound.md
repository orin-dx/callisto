---
callisto-cli: patch
---

**Generated release workflows no longer open a second release PR right after a release.** After a release PR merges, the generated workflow now rebuilds the release PR only once `execute` has tagged the release.

- Packages with `release-trigger = "auto"` were affected: before the new tag existed, the commits just released were counted again and a release PR for the next version opened with nothing new in it.
- Merging that PR would have released a version with no changes. Close any such PR left open.
- Packages released from changesets were not affected.

**Action:** delete `.github/workflows/callisto-release.yml` and regenerate it with `callisto init --workflow`.
