---
callisto-cli: minor
callisto-graph: minor
callisto-model: minor
---

**New `callisto matrix` command**

- Discovers napi and maturin platform targets from `package.json`/`pyproject.toml`.
- Builds a per-triple CI table: host runner, cross-compile flag, artifact name.
- Reports `engines.node`/`requires-python` versions.
