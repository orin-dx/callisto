---
callisto-cli: minor
---

Changelogs group each release's changes by bump, so they are easier to scan.

- Major, Minor and Patch Changes headings
- The release date, plus a link to compare with the previous version when the GitHub repository is known
- A package with no changes of its own, released only because its fixed or linked group was, gets one line saying so

**Upgrading**
- To keep the old single list, set `layout = "flat"` under `[changelog]` in `callisto.toml`.
