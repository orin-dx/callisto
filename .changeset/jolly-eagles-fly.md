---
callisto-cli: patch
---

**Tokenless PyPI publishing works when `PYPI_TOKEN` is empty.** An empty PyPI credential is no longer handed to `twine`, so it falls back to trusted publishing (OIDC) instead of failing on empty credentials.

- Why it matters: workflows that map `PYPI_TOKEN` from an unset or blank secret now publish through trusted publishing.
- How to adopt: configure a trusted publisher on PyPI and leave `PYPI_TOKEN` unset or empty.
