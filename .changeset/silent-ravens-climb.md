---
callisto-cli: minor
---

**Release pull requests run prerelease cycles.** After `callisto pre enter <tag>`, the release workflow keeps a release PR open: each new changeset, or new commit to a `release-trigger = "auto"` package, opens or updates it for the next prerelease, and after `callisto pre exit` it releases the stable versions.

- `release-pr decide` no longer refuses pre mode, so E318 is retired.
- A cycle with nothing new since its last prerelease needs no release PR.
- Works with both Changesets v2 and v3 `pre.json` state.
