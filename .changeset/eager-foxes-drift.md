---
callisto-cli: patch
---

Loading a workspace now fails with a clear error naming both directories when two of them declare the same ecosystem and name, instead of silently routing dependency edges to the wrong package.
