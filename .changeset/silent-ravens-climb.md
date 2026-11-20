---
callisto-cli: minor
---

**Release pull requests run prerelease cycles.** After `callisto pre enter <tag>`, the release workflow keeps opening release PRs: each new changeset or inferred commit opens one for the next prerelease, and after `callisto pre exit` the next one releases the stable versions.

- `release-pr decide` no longer refuses pre mode, so E318 is retired.
- A cycle with nothing new since its last prerelease opens no pull request.
- Works with both Changesets v2 and v3 `pre.json` state.
