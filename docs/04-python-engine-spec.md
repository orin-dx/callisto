# Callisto Specification: Python Engine (`docs/04-python-engine-spec.md`)

---

## 1. Specification Overview & Standard Compliance

The Python Engine in Callisto provides native versioning, dependency tracking, and publication orchestration for Python projects in monorepos.

It adheres to 5 core Python Enhancement Proposals (PEPs):
- **PEP 440**: Version Specifiers (`0.3.2`, `0.3.2a1`, `0.3.2.post1`, `0.3.2.dev1`).
- **PEP 508**: Dependency Requirement Specifiers (`requests[security]>=2.28.0; sys_platform == 'win32'`).
- **PEP 517 / PEP 518**: Build isolation & build-system requirements (`maturin`, `hatchling`, `flit_core`, `setuptools`, `poetry-core`).
- **PEP 621**: Standardized `[project]` metadata table in `pyproject.toml`.
- **PEP 735**: Dependency Groups (`[dependency-groups]`).

---

## 2. Manifest Schema & CST Editing Rules

`PyprojectToml` in `callisto-manifests::python` uses `toml_edit::DocumentMut` for 100% comment, whitespace, and key order preservation.

### Supported Manifest Standards:
1. **PEP 621 Standard**: `[project] version = "..."`, `[project] dependencies = [...]`
2. **Poetry Format**: `[tool.poetry] version = "..."`, `[tool.poetry.dependencies]`
3. **Flit Format**: `[tool.flit.metadata] version = "..."`
4. **Hatch Format**: `[project] version = "..."`
5. **Maturin Format**: `[build-system] build-backend = "maturin"`, `[project]`

### Version Bumping Rules:
When updating `version`:
- Decor (comments, inline suffix comments) attached to the `version` field MUST be preserved.
- UTF-8 BOM (`\u{FEFF}`) headers MUST be stripped on parse and handled cleanly.

---

## 3. Dependency Requirement Specifier Partitioning

When parsing requirement strings from PEP 621 arrays or Poetry tables:
1. Environment markers (anything after `;`) MUST be preserved separately during updates.
2. Extras (anything inside `[...]`) MUST be preserved during version updates:
   - Example: `"requests[security]>=2.28.0"` $\rightarrow$ package name: `"requests"`, extras: `"[security]"`, requirement range: `">=2.28.0"`.

---

## 4. Lockfile Auto-Staging

When `callisto version` runs, the engine auto-detects and stages the following lockfiles if present in the workspace:
- `uv.lock` (UV package manager)
- `poetry.lock` (Poetry package manager)
- `pdm.lock` (PDM package manager)
- `Pipfile.lock` (Pipenv)
