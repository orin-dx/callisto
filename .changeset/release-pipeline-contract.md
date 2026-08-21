---
callisto-model: minor
callisto-graph: minor
callisto-cli: minor
callisto-moon: patch
---

**Release-pipeline / CI Action contract correctness**

- **Breaking:** several `--format json` field names changed or were added (`validate`, `compose-pr-body`, `tag`, `status`, `plan-publish`) — update any scripts parsing this output.
- Re-tagging a release that already exists no longer reports the wrong commit sha.
- The official GitHub Action now actually opens a release PR when changesets are pending — a bug made this step unreachable before.
- GitHub Releases are now correctly marked prerelease for PEP 440 versions too (e.g. `1.2.3a1`), not just SemVer's `-` syntax.
- Release notes now include the real changelog section instead of nothing.
