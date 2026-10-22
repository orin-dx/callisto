---
callisto-cli: patch
---

A failed npm, Cargo or PyPI publish now shows what the publish tool printed to standard output, not only standard error. pnpm prints its errors there, so a failed pnpm publish used to end with `npm publish failed (exit 1):` and nothing after it. The output also appears live in the release log while the publish runs, instead of only after it ends.
