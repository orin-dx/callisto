---
callisto-cli: patch
---

A Python dependency written as a wildcard (`lib==1.*`, `lib==1.2.*`) is no longer rewritten into an exact pin when `lib` is released.

- Before, `lib==1.*` became `lib==2.0.0` on the next release, which silently turned a loose constraint into a hard pin. `lib===1.2.3` also became `lib==2.0.0`.
- Now these ranges are treated like other forms Callisto cannot rewrite mechanically: if the new version is outside the range, `callisto version` stops with `E319` and changes nothing, as it already does for Cargo and npm wildcards.
- If you hit `E319`, edit the range in `pyproject.toml` so it covers the new version, then run `version` again.
