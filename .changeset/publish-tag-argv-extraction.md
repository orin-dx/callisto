---
callisto-graph: minor
callisto-vcs: minor
---

**Fix the durable release executor's registry-publish and tag-creation argv; delete the disused `PublishOrchestrator`/`SubprocessRegistryClient` path**

The durable release executor's `dispatch_registry` (the live `callisto release execute` publish path) was rewritten independently of the older `PublishOrchestrator`/`SubprocessRegistryClient` pipeline and silently dropped real behavior along the way:

- **npm**: it hardcoded plain `npm publish`, regardless of the workspace's actual package manager. A pnpm or yarn workspace published with the wrong tool -- a live, real bug. It now detects `pnpm-lock.yaml`/`yarn.lock`/`bun.lock(b)` and builds the correct `pnpm publish --filter`/`yarn workspace ... npm publish`/`bun publish`/`npm --workspace` invocation.
- **cargo**: `cargo publish` ran with no `--locked` and no check that the on-disk manifest version matched the release plan. Both are restored; a mismatch now fails fast with `GraphError::OnDiskVersionDrift` instead of silently publishing a stale version.
- **pypi**: it built into a throwaway temp directory and uploaded whatever landed there. It now builds into `dist/` and uploads the exact `dist/<normalized-name>-<version>*` glob, matching the older client's intentional scoping.
- **git tag creation**: it inlined its own `git tag --no-sign -a ...` call with no `--` end-of-options guard against a malicious/malformed tag name. It now goes through `callisto_vcs::GitDataSource::create_tag`, which already has that guard -- extended with a new `TagSignPolicy` parameter (`RespectRepoConfig` for the existing non-durable `callisto tag` path, `ForceUnsigned` for the durable executor) so both safety properties hold together instead of one replacing the other.

The corrected argv construction and output classification for all three registry ecosystems now live in a new pure module, `callisto_graph::commands::registry_argv` (no `CommandRunner`/orchestration logic of its own). `PublishOrchestrator`, `SubprocessRegistryClient`, `AlwaysRetryPolicy`, `SystemTimeProvider`, and `parse_retry_after` are deleted: nothing in production ever constructed them (`callisto publish`'s CLI handler is an explicit dry-run-only compatibility preview; `release execute` is the only real effect path), and their retry/backoff/pre-check policy has no equivalent in the durable executor's model, which persists `Attempting` state and relies on a later re-invocation to retry rather than an in-process retry loop.
