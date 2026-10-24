---
callisto-cli: minor
---

A release commit is now checked for what else it changes. Verification (E165) refuses a version commit that touches any path other than manifests, lockfiles, changelogs, changesets, `pre.json` and the decision file, so a release pull request can no longer carry a change to source, a build script, `.npmrc` or `.cargo/config.toml` under the cover of a bot-authored diff. Move such edits to a separate commit.
