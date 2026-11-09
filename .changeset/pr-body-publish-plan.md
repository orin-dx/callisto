---
callisto-cli: minor
---

The release PR shows where merging will publish each package (registry, git tag, GitHub release, assets) and warns up front when the release would be refused, so a broken release is caught before the merge. Turn it on with `compose-pr-body --publish-plan`; the release PR action already does.
