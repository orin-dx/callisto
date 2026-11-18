---
callisto-cli: patch
---

**Release lookups no longer fail on pretty-printed forge responses.** A forge reply whose body contains a blank line, such as indented JSON, is now read whole instead of being cut at the last blank line.

- Why it matters: the truncated body failed to parse, so the lookup errored instead of reporting the real state.
- Adopt: nothing to do, upgrade.
