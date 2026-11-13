---
callisto-cli: patch
---

In pre-release mode, `callisto version` no longer lowers a package that already has a higher prerelease.

- Before, a package whose changeset was already recorded in `pre.json` lost that bump on the next run. If a dependency or a fixed-group sibling then bumped it with a lower severity, it was recomputed from the starting version: `1.1.0-beta.0` became `1.0.1-beta.0`, a version older than the one already published.
- Now the result is raised until it is not behind the current version, so that package goes to `1.1.0-beta.1`.
- If no bump can get past the current version, for example after switching the pre-release tag from `rc` to `beta`, `version` stops with the existing version-regression error instead of writing an older version.
- You do not need to do anything.
