---
callisto-cli: minor
---

**`callisto init` asks for intent and previews the first release**

`init` prints what it detected (ecosystems, packages, `origin`, last tags, binaries), then asks only for versioning mode and, when a package builds a binary, whether to ship binaries (product package, GitHub repository, target triples). It shows the resulting `callisto.toml` and the first release plan from the same derivation as `release --dry-run`, then writes after confirmation. Non-default tag conventions (`{name}-v{version}`, `{name}-{version}`, `v{version}`) are detected and kept. Non-interactive: `--yes --versioning <fixed|independent>`, plus `--artifact-target`, `--forge-repository`, `--product-package` to ship binaries. In a repository with no commit yet, `init` writes the config and skips the preview.

A repository whose root is a single package (`Cargo.toml` with `[package]`, `package.json`, or `pyproject.toml`, no workspace manifest) is now a valid root for every command.

Breaking:
- `init` refuses an existing `callisto.toml`; the `[init]` reconcile flow is gone (an `[init]` table still loads and is ignored).
- A non-terminal run needs `--yes --versioning`; `init --yes` alone errors.
- `init` requires a Git repository with an `origin` remote.
- A forge repository other than `origin`'s GitHub repository is refused.
- The written config holds only answers: no `[changesets]`, `[cascade]`, or `[init]` defaults.
- `InitReport` JSON drops `diff` and adds `config`.
