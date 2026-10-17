---
callisto-cli: patch
---

`callisto add` no longer overwrites an existing changeset when it draws a name that is already taken, and a file Callisto rewrites keeps its permissions (it was left owner-only). A command whose output was cut short because a background process kept its pipes open is now an error instead of a success with empty output. Workspace globs written as `./packages/*` or `packages/*/` now match the same directories as `packages/*`.
