---
callisto-cli: patch
---

Two checks now report what is actually wrong:
- `callisto status --check --strict` now fails on the same warnings `callisto version --strict` does, such as a napi target missing from its fixed group. Without `--strict` these stay warnings.
- `release execute` with a manifest that seals Python files but no `--artifact-dir` now reports E233 and asks for the directory, instead of E232 telling you to drop both flags.
