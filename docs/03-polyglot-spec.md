# Callisto Specification: Multi-Ecosystem Polyglot Release Engine (`docs/03-polyglot-spec.md`)

---

## 1. Scope & Ecosystem Architecture

Callisto is designed as a polyglot monorepo release management engine. This specification defines the architectural contracts for expanding Callisto beyond Rust (`Cargo.toml`) and TypeScript (`package.json`) to natively support **Python**, **Go**, **Java (Maven/Gradle)**, and **C# (.NET)**.

```text
┌────────────────────────────────────────────────────────────────────────┐
│               CALLISTO MULTI-ECOSYSTEM SPECIFICATION MATRIX            │
├─────────┬─────────────────┬───────────────────┬────────────────────────┤
│ ECO     │ MANIFEST FORMAT │ VERSION DRIVER    │ VERSION GRAMMAR        │
├─────────┼─────────────────┼───────────────────┼────────────────────────┤
│ Cargo   │ Cargo.toml      │ ManifestField     │ SemVer 2.0.0           │
│ Npm     │ package.json    │ ManifestField     │ SemVer 2.0.0           │
│ Python  │ pyproject.toml  │ ManifestField     │ PEP 440 (pep440_rs)    │
│ Go      │ go.mod          │ GitTag            │ SemVer 2.0.0 (vX.Y.Z)  │
│ Java    │ pom.xml / prop  │ ManifestField     │ Maven Qualifiers       │
│ C#      │ *.csproj / CPM  │ ManifestField     │ NuGet SemVer           │
└─────────┴─────────────────┴───────────────────┴────────────────────────┘
```

---

## 2. Invariants Across Ecosystems

1. **Concrete Syntax Tree (CST) Preservation**: No text/regex replacement. Manifest modifications use format-native CST editors (`toml_edit` for TOML, `serde_json` with format fingerprinting for JSON, `xmltree` for XML). Comments, inline formatting, and key order MUST be 100% preserved.
2. **Crash-Safe Atomic Writes**: All file mutations pass through `atomic_write` tempfile flush + parent/grandparent directory journal synchronization (`sync_all()`).
3. **Lockfile Auto-Staging**: Bumping versions auto-stages ecosystem lockfiles (`Cargo.lock`, `package-lock.json`, `pnpm-lock.yaml`, `yarn.lock`, `bun.lockb`, `uv.lock`, `poetry.lock`, `pdm.lock`, `Pipfile.lock`, `go.sum`, `gradle.lockfile`, `packages.lock.json`).

---

## 3. Implementation Phasing Strategy

- **Phase 1 (Python)**: `pyproject.toml` CST editor, PEP 440 versioning, PEP 508 dependency partitioning, lockfile auto-staging (`uv.lock`, `poetry.lock`, `pdm.lock`, `Pipfile.lock`).
- **Phase 2 (Go)**: Tag-Driven Versioning (`VersionSource::GitTag`), `go.mod` / `go.work` discovery, directory-prefixed tags (`subpkg/vX.Y.Z`), `go.sum` staging.
- **Phase 3 (Java)**: XML CST editor (`xmltree`) for `pom.xml` and `gradle.properties` parser. Maven qualifier versioning (`1.2.3-SNAPSHOT`).
- **Phase 4 (C#)**: MSBuild XML CST editor for `*.csproj`, `Directory.Build.props`, and Central Package Management (`Directory.Packages.props`).
