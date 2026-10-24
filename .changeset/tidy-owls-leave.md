---
callisto-cli: minor
---

The leftover `[init]` and `[release.profiles]` tables in `callisto.toml` are removed, and a config that still has one now fails to load, naming the key; move a profile's `forge-repository` to `[release]`. The no-op `setup_git_user` action input is removed, and the setup action no longer falls back to building unreleased `main` when a release cannot be installed. The job's OIDC request variables now reach only the package-manager tools, not cargo, git or gh.
