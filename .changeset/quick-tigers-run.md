---
callisto-cli: patch
---

Workspaces with thousands of packages load and plan about a quarter faster: manifests are read and parsed on several threads, and package name lookups no longer slow down as the package count grows.
