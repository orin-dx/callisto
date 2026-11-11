---
callisto-cli: patch
---

`callisto status` and the release preview now see committed changes when your Callisto workspace is a subdirectory of the git repository.

- `callisto status` reported `changedSinceLastTag: false` for packages that had commits since their last tag. It now reports `true`.
- `callisto release --dry-run` no longer misses the warning for changed packages that have no changeset.
- `callisto version` reads manifests and checks for pending changesets at HEAD correctly from a subdirectory workspace.

Nothing to do. A workspace at the repository root was never affected.
