# PR #101 performance / resource review (head 4d05e40d, base 5e452d04)

Verdict: no algorithmic hotspot at Callisto scale (~9 packages, ~31 ops, 4 artifacts). The real costs are (a) unbounded outbound calls and no job timeouts, (b) transient provider errors that abort the run and force a full ~60-90 min pipeline rerun, (c) CI critical-path setup repetition. One incidental correctness finding (X1) looks release-blocking. Ledger checked: no overlap.

## P1

**P1-1 No timeout on most outbound calls; no `timeout-minutes` anywhere. CONFIRMED.**
- `CliCommandRunner::run` is `Command::output()` (crates/callisto-cli/src/runner.rs:19-26): blocks forever, unbounded capture. The 10 MiB cap and deadline exist only in `run_with_timeout_impl` (runner.rs:14,184).
- The durable path uses `run` for: publish argv (`run_argv`, graph/commands/release.rs:743 -> cargo/npm publish, pypi build+twine), `git push` (:833), `gh release create` (:888), `gh release upload` (:960), both `gh api` observations (:1006, :1117), all git. Only `cargo info`/`npm view` (300 s, :759,:794) and `gh attestation verify` (120 s, release_artifacts.rs:18) are bounded.
- `PUBLISH_TIMEOUT_SECS` was deleted in 143e4f77 (#60); only a stale comment remains (runner.rs:12). Pre-existing, but this PR adds new unbounded `gh api`/`gh release upload` calls to the path and makes it the only publish path.
- `grep timeout-minutes .github/workflows` = 0 hits. Release concurrency is `release-${{ github.ref }}` with `cancel-in-progress: false` (callisto-release.yml:16-18), so one hung `execute` holds the group up to the 6 h default and queues every later main push and recovery dispatch behind it.
- Even the bounded probes are generous: 300 s x 4 polls (release.rs:706,725) = ~20 min/package on a hung index.
- Budget: happy path is ~60-100 min (PLAUSIBLE, see P3-1), far below 6 h, so timeouts are about hang detection. Fix: `run_with_timeout` for every effect (publish ~15 min, push ~2 min, `gh api` 60 s, upload ~10 min, probes 30-60 s) and per-job `timeout-minutes` (verify 45, build-artifact 30, execute 60).

**X1 (out of my lane, incidental, release-blocking): trust re-check flips as soon as `cargo publish` creates `target/`. CONFIRMED in code + simulated.**
- `recheck_trust` (release.rs:214-233) compares `evidence != self.prepared.trust`; `GitCommitTrustEvidence` derives `PartialEq` over `allowed_ignored_paths` (vcs/access.rs:38-44), populated from `git status --ignored=matching` (shell.rs:158-167,265-283).
- `cargo publish --manifest-path release-source/...` (registry_argv.rs:68-73, no `--no-verify`) creates `release-source/target/package/...`. I simulated in a scratch clone at 4d05e40d: status output goes from empty to `!! target/`. `target/` is on the allowlist (shell.rs:37), so it is accepted but recorded, so evidence != validated evidence, so `ReleaseIntentStale` before the next op (loop at release_execution.rs:77).
- The e2e fake `cargo publish` only touches a marker outside the worktree (durable_release_e2e_tests.rs:501), so tests cannot see it. Caveat: I could not see `orin-dx/actions/setup-rust`; no `CARGO_TARGET_DIR` is set in-repo. Expected effect: each run publishes one crate then fails. Confirm with a real `cargo publish --dry-run` in a clean checkout, then run recheck. Fix: compare a canonicalized evidence that ignores allowlisted-ignored content, or set `CARGO_TARGET_DIR` outside the worktree.

## P2

**P2-1 Rate-limit/transient errors are classified then discarded; no retry, no jitter, no backoff. CONFIRMED.**
- `RegistryError::RateLimited(Duration)` with parsed Retry-After (registry_argv.rs:29,350-362) is mapped straight to `GraphError::Registry` (release.rs:641-644): the wait is dead data.
- GitHub 403/429/5xx map to `Indeterminate` (release.rs:1222-1228), which is a hard error in dispatch (:872-876, :929-933).
- Registry confirmation is 4 checks over a fixed 2/4/8 s = 14 s (release.rs:706-713), then `RegistryPublishUnconfirmed`. npm replica lag over ~15 s is plausible.
- Each transient blip leaves the op `Attempting` and costs a full recovery dispatch (P2-2). The polling loop is bounded (good) but a single client needs no jitter. Fix: bounded retry (3 attempts, honour Retry-After, cap 5 min) for observation and `RateLimited`.

**P2-2 Recovery pays full `verify` and couples to unrelated main health. CONFIRMED (structure), PLAUSIBLE (magnitude).**
- On `workflow_dispatch` with `release_source_sha`, `version-pr` is skipped (release.yml:74) but `verify` still runs and gates `release-candidate` (`needs.verify.result == 'success'`, :108). It checks out `github.sha` (main tip), not the release source, and runs fmt, clippy, full nextest, wasm, audit serially in one job (:21-69), which CI runs as 5 parallel jobs.
- Recovery MTTR = verify (est. 25-40 min) + plan + 4 LTO builds + assemble + execute. An unrelated flaky test blocks recovery of a half-published production release.
- Measure: `gh run view <id> --json jobs --jq '.jobs[]|[.name,.startedAt,.completedAt]'`. Fix: skip verify when `inputs.release_source_sha != ''`, or split it into parallel jobs.

## P3

**P3-1 Critical path repeats a full toolchain + debug build 6 times serially. CONFIRMED structure / PLAUSIBLE cost.**
- `setup-callisto` (action.yml:104) runs `cargo build -p callisto-cli` in verify, version-pr, plan, build-artifact x4, build, execute; chain is verify -> version-pr -> candidate -> plan -> build-artifact -> build -> execute.
- `moonrepo/setup-toolchain` is installed in jobs that never call moon. `extractions/setup-just` in build-artifact is unused by the new script.
- `fetch-depth: 0` x2 in plan/build-artifact/build/execute (local pack 281 MiB). Non-shallow is required only where callisto runs against the source: plan and execute (shell.rs:123-130). No build.rs in the repo, so orchestration and build-artifact release-source checkouts can be shallow.
- `build` exists to hash 4 files (`artifact-manifest`) yet pays a whole job setup (est. 3-6 min). Security-motivated split (no secrets); the cost is real.
- Measure: step durations of "Install or Build Callisto Binary" per job in a recent run.

**P3-2 Release builds are uncached by design; CI preflight repeats them. PLAUSIBLE.**
- `lto = true`, `codegen-units = 1` (Cargo.toml:47-50). `release-source/target` is outside rust-cache's workspace, so third-party crates rebuild each release (plausibly intentional against cache poisoning; keep, but it is ~5-10 min).
- `cargo install cross --locked --version 0.2.5` compiles from source every run (build-release-artifact.sh:42) in the musl leg, in both preflight and release, plus a Docker pull. Use a prebuilt (taiki-e/install-action is already used) or native musl-tools.
- Preflight = 4 identical LTO builds per PR push (intended "same recipe"); consider path-gating for PRs that touch neither Cargo.* nor .github/scripts.
- `save-if: "true"` with one shared-key in all jobs (setup-callisto action.yml:20-21) makes parallel jobs race to save after each release bump changes Cargo.lock. Preflight has no key, so the three ubuntu legs likely share one job-id key (external action unseen). Measure: post-step durations.

**P3-3 Duplicate test run. PLAUSIBLE.** `just release-workflow-behavior` (callisto-ci.yml:111) reruns `cargo test -p callisto-cli --test durable_release_e2e_tests` after `just test-ci`, which already ran it (`--workspace --all-features`). The feature set differs (default vs all), so it likely recompiles graph and cli in the test profile, on 2 OSes. Confirm by the "Compiling callisto-graph" lines in that step's log.

**P3-4 Observation repeated, serial, uncached. CONFIRMED, small.**
- Per Cargo package: `cargo info` pre-publish (release.rs:565), post-publish poll (:658), and again in the receipt pass (cli release.rs:421 -> release.rs:252-268). The receipt pass re-observes all ~31 ops (about 9 x `cargo info` + 13 x `gh api`, est. 15-30 s), and recovery adds a second full pass (release_execution.rs:97-117).
- Each artifact slot fetches the same `releases/tags/<tag>` 3x per dispatch (release.rs:922, :938, :971) and once per pass, so 12+ identical GETs for 4 slots. `gh attestation verify` runs 4x serially (release_artifacts.rs:74-90, est. 5-20 s). Rate budget is fine: about 100 REST calls vs the 1000/h GITHUB_TOKEN limit.
- Per op, `recheck_trust` = `GitAccess::discover` (gix open, discarded because the shell backend is used) plus 6-7 git spawns (shell.rs:107-168, release.rs:216); ~62 calls per run is a few seconds. The suspected `git status --ignored=matching` cost is disproved: 23 ms on this repo with a huge `target/`, because matching mode does not descend.
- Two `Workspace::load` per command (cli release.rs:136,336 + graph release.rs:280,350) are intentional per the module doc.

**P3-5 `reconcile_release_execution` is path-exponential. PLAUSIBLE, irrelevant today.**
- `prerequisites_satisfied_transitively` removes `id` from `visited` on exit (release_execution.rs:291-313), so it enumerates paths, not nodes. It is evaluated for every Pending op each loop iteration though only `.first()` is used (:74), plus ~3 `validate_for_intent` BTreeSet builds per op.
- Callisto's DAG is ~370 visits per pass. A dense layered DAG of k crates costs up to 2^(k-1) (k=25 => ~16M per op). Measure by timing `reconcile_release_execution` on a synthetic complete-DAG intent. Fix: single topological DP.

**P3-6 Artifact bytes and state I/O are fine.** State is saved 2x per op (~62 fsyncs, <1 s); reconstruct also saves on `Absent` with no change (release_execution.rs:111-115), trivial. Each asset hops 5 times (upload, download, re-upload in `build`, download, `gh upload`); `build` could upload only manifest.json and execute could use `pattern: release-build-*` with `merge-multiple`. `compression-level: 0` for .tar.gz saves seconds. `callisto` runs as a debug build (sha2 at opt-level 0) hashing ~30 MB once or twice, est. seconds.

**P3-7 config/resolve.rs adds nothing to non-release commands.** `raw.release.map(resolve_product_release)` (resolve.rs:411) runs only if `[release]` exists; the P^2 profile checks have P<=2. Workspaces that have `[release]` (Callisto) now fail every command on invalid release config (coupling, not perf).

**P3-8 Concurrency window. PLAUSIBLE.** A group holds one running plus one pending run; a pending release-merge run is replaced by the next push, whose `github.sha` is not a release commit (release.yml:127), so the release is silently skipped. The 60-100 min chain widens the window.

## Done well
1. The four artifact builds are a parallel `fail-fast: false` matrix (was one serial `just build`), and the recipe script is stub-tested.
2. Artifact hashing is streamed through a 64 KiB buffer (model/release.rs:372-387), never whole-file in memory; `Attempting` is persisted before dispatch; the workspace lock is non-blocking `try_lock_exclusive` (no hang).
3. Decision derivation batches `git cat-file --batch` (one spawn), and `plan`/`reconcile` do no provider calls.
