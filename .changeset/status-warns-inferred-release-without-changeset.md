---
callisto-cli: minor
---

`status` now warns when a package has a pending release inferred from commits but no changeset names it. The release pull request reads changesets only, so it would never release that package. `status --check --strict` fails on it.
