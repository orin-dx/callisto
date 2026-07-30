# callisto-manifests

Concrete Syntax Tree (CST) manifest editors and crash-safe atomic writer for Callisto.

## Overview

`callisto-manifests` provides comment-preserving manifest inspection and mutation:

- **TOML Editing**: Manipulates `Cargo.toml` and `pyproject.toml` using `toml_edit` to preserve key order, whitespace, and user comments.
- **JSON Editing**: Manipulates `package.json` preserving indentation style (`tabs` vs `spaces`) and key order via `serde_json`.
- **Atomic Disk Writes**: Enforces crash-safe atomic file replace (`NamedTempFile` flush, `sync_all`, `rename`).

## License

GNU Affero General Public License v3.0 (`AGPL-3.0-only`).
