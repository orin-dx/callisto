---
callisto-cli: patch
---

**Prerelease reruns keep new work.** In a pre cycle, a commit or group change that arrives after a changeset was already applied now releases the next prerelease instead of being dropped.

- The next prerelease keeps the severity the earlier changeset set, so a major cycle stays `2.0.0-beta.N`.
- Fixed and linked group members and dependents of such a package are bumped with it.
- A rerun with nothing new still plans nothing.
