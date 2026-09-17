# callisto-manifests

Concrete Syntax Tree (CST) manifest editors and crash-safe atomic writer for Callisto.

## Overview

`callisto-manifests` provides comment-preserving manifest inspection and mutation:

- **TOML Editing**: Manipulates `Cargo.toml` and `pyproject.toml` using `toml_edit` to preserve key order, whitespace, and user comments.
- **JSON Editing**: Manipulates `package.json` preserving indentation style (`tabs` vs `spaces`) and key order via `serde_json`.
- **Atomic Disk Writes**: Enforces crash-safe atomic file replace (`NamedTempFile` flush, `sync_all`, `rename`).

## License

Functional Source License, Version 1.1, MIT Future License (`FSL-1.1-MIT`).
