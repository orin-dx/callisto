---
callisto-cli: patch
---

Fixes four mismatches with the documented behaviour:
- `[[package]]` and `[[package-set]]` `match` now accept only `cargo/`, `npm/` or `pypi/` prefixes; any other prefix is E317.
- A `workspace:*`-style pnpm dependency that matches no workspace package now warns instead of staying silent.
- Under `--format json`, `error.path` is set for init, apply, artifact, release-input and config file errors that name a file.
- `pre exit` and `pre enter` report E114 for a baseline that is not a valid version, instead of E212.
