---
callisto-cli: minor
---

**Shorter release configuration.** List your build targets once and let Callisto name each asset, and drop `forge-repository`.

- How to adopt it: replace repeated `[[release.artifact]]` blocks for the product with `targets = [...]` and `asset-name = "tool-{target}.tar.gz"` under `[release]`. Keep `[[release.artifact]]` for another package's artifact or an irregular name such as a Windows `.zip`.
- `forge-repository` is optional: Callisto uses the `origin` GitHub repository, which every release already required it to equal.
- `callisto init` now writes `targets` and one `asset-name` template, named after the single binary or after the package when it has several.
- Limits: `{target}` is the only placeholder. A target named by both `targets` and a `[[release.artifact]]` of the product is E197.

**Breaking:** `callisto init` no longer asks for or accepts `--forge-repository`.

**Action:** drop `--forge-repository` from scripted `init` calls.
