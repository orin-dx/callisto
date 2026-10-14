---
callisto-cli: patch
---

Cargo dependencies declared under `[target.<cfg>.dependencies]` are now followed: bumping a package releases its target-specific dependents and rewrites their pins, instead of leaving a stale pin that broke the workspace.
