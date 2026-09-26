---
callisto-cli: patch
---

callisto release --dry-run and callisto release plan now include a diagnostics array in their JSON output when an artifact owner package is selected without its product package, instead of only printing a stderr warning.
