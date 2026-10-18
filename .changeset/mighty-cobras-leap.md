---
callisto-cli: patch
---

A nested npm workspace that declares `workspaces` in its `package.json` and has no lockfile now publishes from its own directory, and a lockfile beside a package no longer takes it out of its workspace. `npm view` and a publish by directory run in that directory too, so the version looked for and the version published come from the same `.npmrc`. A package that moves to a different `package.workspace` between two commits is verified against each commit's own workspace, and a directory that cannot be opened for syncing after a write is reported instead of skipped.
