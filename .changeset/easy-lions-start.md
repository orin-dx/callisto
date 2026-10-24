---
callisto-cli: minor
---

`callisto init --yes` now works with no other flags: packages version independently unless `--versioning fixed` is given, and a GitHub repository gets the release workflow unless `--no-workflow` is given. A single package is no longer asked how to version. If the unrequested workflow cannot be built, such as offline, `init` skips it with a warning instead of failing. After writing the workflow, `init` lists the repository secrets to add and the first commands to run, and the publishing guide has new sections on first publishes and on pulling a release.
