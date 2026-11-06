---
callisto-cli: patch
---

A local `callisto release` could miss a package that changed since its version commit (E320) when an earlier package in the release spans more than one directory and had already changed. It now compares every directory of every package.
