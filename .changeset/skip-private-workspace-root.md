---
callisto-cli: minor
---

A private npm workspace root is no longer treated as a package.

- A root `package.json` that is `private` and lists `workspaces` is skipped even when it has a version such as `0.0.0`, so it stops appearing in `status` and in release plans. It only holds scripts and tooling, is never published, and nothing depends on it by version.
- `status` prints one line when it skips a root, so it never disappears silently.
- To version the root anyway, name it in `callisto.toml` with a `[[package]]` block, for example `match = "npm/monorepo"`.
