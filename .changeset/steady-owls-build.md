---
callisto-cli: patch
---

A generated build matrix builds `x86_64-apple-darwin` on `macos-latest`, since GitHub has retired the `macos-13` runner it used. The proto plugin now installs the Intel macOS and arm64 Linux release binaries.
