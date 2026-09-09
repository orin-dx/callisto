---
callisto-vcs: patch
---

`GitAccess`'s four READ methods (`head_sha`, `list_tags`, `resolve_commit`, `commits_since`) each independently reimplemented the same three-step native-`gix`-then-shell-fallback policy inline. Factored the shared policy into a private `read_with_fallback` helper and rewrote all four in terms of it via closures.

The WRITE methods (`create_tag`, `create_floating_major`) are untouched: they keep their existing authoritative-native-result inline logic and do not call the new helper, preserving the module doc's deliberate reads-fall-back-on-any-error / writes-never-fall-back distinction. No behavior change for any caller.
