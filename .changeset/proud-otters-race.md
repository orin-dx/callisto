---
callisto-cli: minor
---

Workspaces below the repository root are now supported for npm, pnpm and uv as they are for Cargo. A nested workspace decides which packages beneath it are packages by its own `workspaces`, `pnpm-workspace.yaml` or `[tool.uv.workspace]`, a private versionless `package.json` that holds it is not a package, and `callisto version` refreshes and stages the lockfile nearest to each package instead of only the one at the root.
