---
callisto-cli: minor
---

**Release pull requests run prerelease cycles.** `release-pr decide` no longer refuses pre mode (E318 is retired): each new changeset or commit in a cycle opens a release PR for the next prerelease, and `pre exit` opens the one for the stable release.

- A cycle with nothing new since its last prerelease opens no pull request.
- Prerelease version commits verify as release authority like any other.
