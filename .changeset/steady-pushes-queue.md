---
callisto-cli: patch
---

The release workflow `callisto init` generates for a simple workspace now runs one release at a time, so two pushes to the default branch in a row no longer race to tag and publish the same versions.
