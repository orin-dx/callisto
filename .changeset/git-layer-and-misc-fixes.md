---
callisto-conventional: minor
callisto-vcs: patch
callisto-graph: patch
callisto-model: patch
callisto-format: patch
callisto-manifests: patch
---

**Git-access layer decoupling, performance, and small correctness fixes**

- **Breaking (library consumers only):** `callisto-conventional`'s public functions no longer take a `callisto-vcs` type directly.
- A `BREAKING CHANGE:` commit footer is now parsed correctly.
- Commits merged in from another branch are no longer missed when scanning history since the last tag.
- `^1.2.3` no longer incorrectly matches a prerelease like `1.9.0-alpha.1`.
- Compound Cargo dependency ranges are now rewritten correctly during a version bump.
- PEP 440 prerelease detection and version-range matching for Python packages is now correct.
- Python dependency names are now matched with PEP 503 normalization (e.g. `My-Package` and `my_package` are treated as the same dependency).
- npm publish now auto-detects your workspace's package manager (pnpm/yarn/bun/npm) instead of assuming npm.
- A few errors that used to share one error code now have their own, more specific code.
- `status`, `plan-snapshot`, `plan-publish`, and `plan-version` are now faster on large repos — the git repository is no longer rediscovered per package.
- Tag-existence checks now use a cached index instead of one lookup per release.
