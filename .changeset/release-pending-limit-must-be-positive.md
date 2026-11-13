---
callisto-cli: patch
---

`callisto release pending --limit 0` is now a usage error (exit 2) instead of reporting that nothing is pending.

- Before, `--limit 0` examined no release commits and printed the same result as a workspace with no unfinished release.
- Now `--limit` must be at least 1.
- You do not need to do anything unless a script passes `--limit 0`.
