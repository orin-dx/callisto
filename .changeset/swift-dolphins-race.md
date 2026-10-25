---
callisto-cli: patch
---

A repository whose packages all sit in subdirectories, with no manifest at the top, can now be set up: `init` uses the repository root and writes `callisto.toml`, which marks the root from then on. Before, every command stopped with E030.
