---
callisto-cli: minor
---

In pre-release mode, `version` no longer bumps a package again when its only changesets are already recorded in `pre.json`: a rerun with nothing new changes nothing, and `status` stops reporting it as pending. A new changeset produces the next prerelease. `release-pr decide` now refuses pre-release mode with E318 and a local-release fix, instead of opening a pull request that cannot release.
