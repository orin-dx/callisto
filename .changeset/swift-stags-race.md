---
callisto-cli: minor
---

**`[[package-set]]` is folded into `[[package]]`.** One table now takes an exact name or a glob, and every field, `pre-major-inference` included, applies the same either way.

- Why it matters: `pre-major-inference` on a `[[package-set]]` was ignored; on a glob `[[package]]` it now takes effect.
- How to adopt it: rename each `[[package-set]]` to `[[package]]`; its `match` already holds a glob. A `match` with any of `*?[]{}` is a glob.
- Limits:
  - An exact rule wins over any glob, then the first glob in declaration order. Fields are not merged across rules.
  - A renamed `match` with no glob character is now exact. It outranks globs, and for a private npm workspace root it makes that root a package.
  - `previous-tag-templates` names one package's old tags, so a glob that sets it is E338.

**Breaking:** `[[package-set]]` is no longer a key. A `callisto.toml` that still has one fails with E325 and a message telling you to rename it.

**Breaking:** the `package-set-matched-nothing` diagnostic code is gone. A glob that matches no package now reports `package-rule-matched-nothing`, the code exact rules already used.

**Action:** rename `[[package-set]]` to `[[package]]` in `callisto.toml`, and match `package-rule-matched-nothing` in anything that read `package-set-matched-nothing` from `--json` output.
