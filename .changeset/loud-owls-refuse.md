---
callisto-cli: minor
---

Callisto now stops, naming the file, when a workspace manifest is malformed instead of guessing which packages it contains. An unreadable or unparseable root `Cargo.toml`, `pyproject.toml`, `package.json` or `pnpm-workspace.yaml`, a `members`, `exclude`, `workspaces` or `packages` entry that is not a valid list of globs, and a root Cargo or npm manifest that cannot be loaded for inheritance all fail the command; before, they silently admitted every directory or dropped inherited versions. A package that sits behind a symbolic link is rejected, because Git cannot stage a change to it. A leading byte-order mark in these files is accepted.
