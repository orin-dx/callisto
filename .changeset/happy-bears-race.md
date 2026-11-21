---
callisto-cli: patch
---

**A release that publishes only some of a package's manifests is no longer refused for a dependency cycle among the others.** A package with both a Cargo.toml and a package.json that publishes only to crates.io now plans its crate publishes even when the npm manifests depend on each other in a loop. Dependencies declared by a manifest the release does not publish no longer order any publish, and a cycle between published manifests still fails with E104.
