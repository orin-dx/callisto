---
callisto-cli: minor
---

The `callisto-format` and `callisto-vcs` crates are merged into `callisto-model`, and `callisto-conventional` and `callisto-changelog` into `callisto-graph`. The CLI is unchanged; library users import `callisto_model::format`, `callisto_model::vcs`, `callisto_graph::conventional` and `callisto_graph::changelog`.
