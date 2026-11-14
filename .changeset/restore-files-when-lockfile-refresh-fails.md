---
callisto-cli: patch
---

When `callisto version` cannot refresh a lockfile (`E208`), it now puts the manifests, changelogs and `pre.json` back as they were, so the retry the error asks for works.

- Before, the failure left the bumped manifests and new changelog sections in the working tree with the changesets still in place. Re-running `version` was then refused with `E121` ("looks like an interrupted `version` run"), and its advice was to commit the half-applied release.
- Now nothing is left changed after an `E208`. Fix the cause (for example a dependency range that no longer matches the new version) and run `version` again.
- You do not need to do anything.
