---
callisto-cli: patch
---

- Release PR commits now fail when a Git filter such as CRLF conversion changes a staged file's bytes, and a staged symlink or submodule is rejected as an unsupported mode.
- The setup action now fails when it cannot list release tags to resolve its pinned version, instead of installing the latest release.
