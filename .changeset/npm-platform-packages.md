---
callisto-cli: minor
---

**npm platform packages release with their owner**

An `os`/`cpu` package listed in another package's `optionalDependencies` is now part of that owner: same version, no tag, pins kept in sync, published with `npm publish <dir>` before the owner. Covers napi addons and esbuild-style CLIs, including `npm/<platform>` dirs outside workspace globs. No `[[fixed-group]]` entry needed.

Breaking:
- Release intents are schema v4; re-plan intents made by older versions.
- A dir with both `Cargo.toml` and `package.json` uses the `package.json` name as its npm release id (`npm/michi-node` → `npm/@orin-axi/michi`). Update `--package` selections and re-emit decision files.
- Changesets must name the owner, not a platform package.
