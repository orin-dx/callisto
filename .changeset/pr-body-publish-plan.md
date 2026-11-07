---
callisto-cli: minor
---

The release PR body now lists where merging will publish each package (registries, git tag, GitHub release, assets), planned from the versioned tree with no registry lookup. If that plan would be refused, the body leads with a caution naming the error. `compose-pr-body --publish-plan` turns it on, and the release PR action passes it.
