---
callisto-cli: patch
---

Generated artifact workflows include every supported Cargo binary; npm and Python executables remain registry releases unless a custom workflow builds their artifacts. `callisto init` now reports E304 when an npm or Python executable is chosen as the product package, and suggests `--no-workflow` for a registry-only release.
