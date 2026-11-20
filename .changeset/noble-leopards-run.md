---
callisto-cli: patch
---

**The build-matrix release workflow now passes `NPM_TOKEN` and `PYPI_TOKEN` to npm and twine.** A token-based npm or PyPI publish from that workflow now authenticates, and a workflow with no token keeps working with trusted publishing.

- `NPM_TOKEN` becomes an `.npmrc` entry that reads it, and `PYPI_TOKEN` becomes the twine username and password, as the simple workflow already did.
- An unset or empty secret configures nothing, so npm and twine fall back to OIDC.
- Tokenless publishing for npm and PyPI is documented in `docs/publishing.md`. npm trusted publishing needs Node 22.14 or newer and npm 11.5.1 or newer, and hosted OIDC publishing is not yet verified against the real registries.
