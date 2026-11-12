---
callisto-cli: patch
---

Fixes two checks that misbehaved when the Callisto workspace is a subdirectory of the git repository (`repo/ws`). Both compared repository-relative git paths with workspace-relative ones.

- `callisto version` now refuses to run again with E121 (exit 1) after an interrupted run. If a first `version` staged a bump and you then restored the working tree to HEAD, the second run used to exit 0 with a misleading "No pending changesets" warning. This matches the behavior when the workspace is at the repository root.
- `callisto release plan --from-release-commit` no longer fails with E165 "release-decision file was not written by this commit" for a valid release commit. Edits outside the workspace are still refused.
- That command still cannot succeed from a subdirectory workspace. `release` requires the repository root to be the workspace root (E170), and it now reports E170 instead of the misleading E165.
- You do not need to do anything, unless you hit either problem. If you want to release, move the workspace to the repository root.
