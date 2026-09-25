---
callisto-cli: patch
---

In pre-release mode, `version` records the changesets it used in `pre.json`, so a rerun with no new changeset changes nothing instead of bumping the prerelease number and repeating changelog entries.
