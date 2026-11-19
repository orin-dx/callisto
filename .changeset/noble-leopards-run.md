---
callisto-cli: patch
---

**The build-matrix release workflow now passes `NPM_TOKEN` and `PYPI_TOKEN` to npm and twine.** A token-based npm or PyPI publish from that workflow now authenticates, and a workflow with no token keeps working with trusted publishing.

- `NPM_TOKEN` becomes an `.npmrc` entry that reads it, and `PYPI_TOKEN` becomes the twine username and password, as the simple workflow already did.
- An unset or empty secret configures nothing, so npm and twine fall back to OIDC.

**Tokenless trusted publishing is documented.** `docs/publishing.md` has a recipe for npm and PyPI with no stored token.

- npm trusted publishing needs Node 22.14 or newer and npm 11.5.1 or newer.
- Bind the npm trusted publisher to the release workflow file.
- Keep read access for private dependencies separate from the publish step.
- Hosted OIDC publishing is not yet verified against the real registries.

**A registry `url` does not override native routing.** `@scope:registry` in `.npmrc` still routes a scoped npm package, and Callisto adds no destination allowlist.

**Action:** to adopt the fix, regenerate the build-matrix workflow with `callisto init --workflow`.
