# Callisto Specification: Go Engine (`docs/05-go-engine-spec.md`)

---

## 1. Specification Overview & Tag-Driven Architecture

Unlike Rust (`Cargo.toml`), TypeScript (`package.json`), or Python (`pyproject.toml`), Go modules do **not** store a version string inside `go.mod`.

Versioning in Go is strictly **Git Tag-Driven** (`VersionSource::GitTag`).

---

## 2. Monorepo Tagging Conventions

In Go monorepos containing multiple module directories (e.g. `api/go.mod`, `services/auth/go.mod`):

1. **Root Module Tag**: `vX.Y.Z` (e.g. `v1.2.3`).
2. **Submodule Tag**: `directory/vX.Y.Z` (e.g. `api/v1.2.3`, `services/auth/v1.2.3`).
3. **Major `v2+` Bumps**: When a Go module bumps to `v2` or higher:
   - Module path in `go.mod` MUST update suffix: `module github.com/user/repo/api/v2`.
   - Tag format: `api/v2/v2.0.0`.

---

## 3. Go Workspaces (`go.work`) & Lockfiles

- **`go.work` Discovery**: Callisto discovers workspace modules declared inside `go.work` (`use (...)` directives).
- **Lockfile Auto-Staging**: `go.sum` is auto-detected and staged in `callisto-graph::apply`.
