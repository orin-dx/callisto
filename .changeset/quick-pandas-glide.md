---
callisto-cli: patch
---

callisto completions and callisto schema no longer crash when piped into a command that closes its input early; they now exit cleanly.
