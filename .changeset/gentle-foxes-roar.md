---
callisto-cli: patch
---

**Paths starting with a colon are no longer refused as ignored.** A plan that writes a file such as `:core/Cargo.toml` was refused when `.gitignore` listed `core/`, because the ignore check read the colon as Git pathspec magic. The check now reads every path literally, so only files Git really ignores are refused.
