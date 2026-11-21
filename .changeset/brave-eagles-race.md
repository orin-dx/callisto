---
callisto-cli: patch
---

**Python ranges keep their precision when a bound has a suffix.** A dependent requirement whose bound carries a post, dev or pre-release suffix is now rewritten at the precision it was written with, in any epoch.

- `>=1.2.3.post1,<1.3` becomes `>=1.2.4,<1.3` instead of widening to `<2`.
- `~=1.2.post1` becomes `~=1.2` instead of narrowing to `~=1.2.1`.
