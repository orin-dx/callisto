---
callisto-cli: minor
---

A private npm workspace root is no longer treated as a package.

- A root `package.json` that is `private` and lists `workspaces` is skipped, so it stops appearing in `status` and in release plans. Many project templates give that root a version such as `0.0.0`, and until now Callisto listed it as a package even though it only holds scripts and tooling, is never published, and nothing depends on it by version.
- `status` prints one line when it skips a root, so it never disappears silently.
- To version the root anyway, add a `[[package]]` block to `callisto.toml` with `match = "npm/<name>"`, where `<name>` is the root's `package.json` name, such as `match = "npm/monorepo"`.
