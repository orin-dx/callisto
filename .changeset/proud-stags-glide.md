---
callisto-cli: patch
---

**`pre-major-inference` now applies to `[[package-set]]` globs.** A `0.y` package matched only by a glob rule gets the policy instead of silently running with `off`.

- **Why it matters:** breaking commits on such a package kept their full bump even though the config said otherwise.
- **How to adopt:** nothing to change; a package-set rule with `pre-major-inference = "conservative"` or `"conservative-feat"` now takes effect.
- **Limits:** an exact `[[package]]` rule still wins over a glob as a whole rule; fields are not merged.
