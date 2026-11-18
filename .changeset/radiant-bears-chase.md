---
callisto-cli: patch
---

**Release and registry lookups no longer fail when a GitHub or PyPI response body contains a blank line.** Such a reply, for example indented JSON, is now read whole instead of being cut at the last blank line.

- Why it matters: the truncated body failed to parse, so the lookup errored instead of reporting the real state.
- Adopt: nothing to do, upgrade.
