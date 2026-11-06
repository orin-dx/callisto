---
callisto-cli: patch
---

`just check-api` and the Dependency & API Hygiene job now run on every pull request and compare the public API with the published crates, allowing the change the pending changesets announce. A breaking change without a `minor` changeset fails the pull request instead of the release PR.
