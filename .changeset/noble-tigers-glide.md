---
callisto-cli: patch
---

The generated build-matrix release workflow builds and places napi addons only for packages in the release. An unrelated or misconfigured napi addon no longer adds builds or fails the plan job of a release that does not include it.
