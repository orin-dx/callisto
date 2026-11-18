---
callisto-cli: minor
---

**`status --since <ref>` catches a pull request that forgot its changeset.** Run `callisto status --check --since origin/main` in CI to fail when a package changed on the branch and no changeset names it.

- A package whose bump is inferred from commits passes without one.
- A change that needs no release takes a changeset with severity `none`.
