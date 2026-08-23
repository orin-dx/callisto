---
callisto-graph: patch
---

**Publish order now accounts for dev-dependencies between same-batch packages**

- **Fixed:** `cargo publish` (run without `--no-verify`) rebuilds the packaged tarball to verify it, which needs every declared dependency -- including `[dev-dependencies]` -- resolvable from the registry. Publish ordering previously ignored dev-dependency edges entirely, so a package with a dev-dependency on a same-batch sibling could be published before that sibling, failing with an unresolvable version requirement.
- Dev-dependency ordering is best-effort, not a hard requirement: a legitimate mutual dev-dependency between two otherwise-unrelated packages no longer risks hard-failing the whole publish plan -- it falls back to the previous (non-dev) ordering for that case instead.
