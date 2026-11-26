---
callisto-cli: patch
---

**Git no longer hangs on a credential prompt or misreads a translated message.** Callisto runs every git command with credential prompts off and in the C locale.

- A missing credential fails the command instead of waiting for input that never comes.
- With `color.grep=always` in your git config, Callisto no longer misses which package owned an older bare tag.
- An `origin` push URL containing a line break is refused (E132) before any effect, instead of being shortened to a different URL.
