---
callisto-cli: minor
---

Callisto now stops, naming the file, when a workspace manifest is malformed instead of guessing which packages it contains. An unreadable or unparseable root `Cargo.toml`, `pyproject.toml`, `package.json` or `pnpm-workspace.yaml`, and a `members`, `exclude`, `workspaces` or `packages` entry that is not a valid list of globs, fail the command; before, they silently admitted every directory, so vendored copies and excluded crates became releasable packages. Locating the workspace root also no longer skips a malformed manifest and settles on a different root.
