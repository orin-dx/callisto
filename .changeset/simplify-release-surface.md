---
callisto-cli: minor
---

**One build: every ecosystem and commit inference always compiled in**

Commit inference now ships in the release binary. It runs only for packages with `release-trigger = "auto"`; the default `changeset` trigger is unchanged.

Breaking:
- Removed Cargo features `cargo`, `npm`, `inference` (`callisto-cli`, `callisto-graph`) and `cargo`, `npm`, `pypi`, `go`, `maven`, `nuget`, `deno` (`callisto-manifests`).
