---
callisto-cli: patch
---

**Releases reach `origin` exactly where `git push` would.** Callisto now hands git the remote's name, so `insteadOf`, `pushInsteadOf` and a separate `pushurl` apply once, the way git applies them.

- Local-path, `file://` and self-hosted remotes work for tags; only a remote URL carrying a password, or a user name over HTTP(S), is refused (E132).
- `callisto init` shows origin's GitHub repository instead of its URL, so a URL with credentials is never printed.
- Git runs with credential prompts off and in the C locale, so a missing credential fails instead of hanging and a translated git message cannot be misread.
- With `color.grep=always` in your git config, Callisto no longer misses which package owned an older bare tag.

**Action:** A release intent planned by Callisto 0.22.0 or earlier is refused by `release execute` (E124, naming the release inputs). Plan the release again with this version, then execute the new intent.
