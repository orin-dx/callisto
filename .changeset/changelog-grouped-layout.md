---
callisto-cli: minor
---

Changelog sections are now grouped into Major, Minor and Patch Changes, with the release date and, when the GitHub repository is known (from `[release] forge-repository` or the `origin` remote) and the previous version has a tag, a compare link under the version heading. A fixed group's GitHub release notes keep each member's entries under the bump group they were recorded in. Set `layout = "flat"` under `[changelog]` in `callisto.toml` to keep the old single-list format.
