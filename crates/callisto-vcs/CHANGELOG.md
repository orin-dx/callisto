# callisto-vcs

## 0.7.2

- Released together with the `workspace` fixed group.

## 0.7.1

- Released together with the `workspace` fixed group.

## 0.7.0

- **Fix three error/diagnostic messages that told the operator the wrong cause**
  
  - `callisto-vcs`: `ReleaseWorkspaceLock::acquire` reported every lock-acquisition failure -- permission denied, disk full, a filesystem that doesn't support `flock`, or any other I/O error -- as "another Callisto release already holds this workspace lock". It now distinguishes true contention (matched against `fs2::lock_contended_error()`'s `ErrorKind`, the same signal `fs2` itself uses) from other I/O failures, and reports the real underlying cause for the latter instead of misattributing it to a held lock.
  - `callisto-graph`: `IdentityResolver::resolve` collapsed both an unsupported-ecosystem case and a genuine `PackageId` parse failure into `GraphError::AmbiguousName`, which is semantically wrong for both (neither is a name-ambiguity problem) and discarded the real reason. Two new variants -- `UnsupportedIdentityEcosystem` and `PackageIdentifierParse` (carrying the underlying `PackageIdParseError`) -- now report each failure's own accurate cause.
  - `callisto-cli`: `callisto add --package name:severity` rejected an invalid severity with "Must be patch, minor, or major.", omitting `none`, which `Severity::from_str` has always accepted. The message (and the `--package` flag's help text) now lists all four accepted values.
- **Bump `gix` 0.86 -> 0.87 to drop the fully-yanked `bisync` transitive dependency**
  
  `gix` 0.86's dependency chain (`gix` -> `gix-protocol` 0.64 -> `bisync` ^0.3.0) pulled in `bisync`, every published version of which (0.1.0 through 0.3.1) is now yanked from crates.io -- there is no non-yanked version to pin instead. `gix-protocol` 0.65.0 already replaced `bisync` with `async-trait`/`futures-lite` for its async-generic code generation, so bumping `gix` to 0.87 (which resolves to `gix-protocol` 0.65.1) removes `bisync` from the dependency tree entirely (confirmed: zero occurrences in `Cargo.lock`).
  
  This does not itself fix `just check-api` (`cargo semver-checks check-release`): that command always runs `cargo update` on whatever baseline it builds (the last version published to crates.io, or a specified git revision), and every currently-published version of `callisto-vcs`/`callisto-cli` still requires the old, now-unresolvable `gix` 0.86 chain. `cargo semver-checks` has no flag to skip that baseline `cargo update`, and Cargo itself has no mechanism to force-accept a yanked version during fresh dependency resolution. This is a one-time transition gap: the next version published with this fix becomes the new baseline for all future `check-api` runs, at which point the tool resolves cleanly again since the fixed baseline never touches `bisync`.
- `GitAccess`'s four READ methods (`head_sha`, `list_tags`, `resolve_commit`, `commits_since`) each independently reimplemented the same three-step native-`gix`-then-shell-fallback policy inline. Factored the shared policy into a private `read_with_fallback` helper and rewrote all four in terms of it via closures.
  
  The WRITE methods (`create_tag`, `create_floating_major`) are untouched: they keep their existing authoritative-native-result inline logic and do not call the new helper, preserving the module doc's deliberate reads-fall-back-on-any-error / writes-never-fall-back distinction. No behavior change for any caller.
- **Fix the durable release executor's registry-publish and tag-creation argv; delete the disused `PublishOrchestrator`/`SubprocessRegistryClient` path**
  
  The durable release executor's `dispatch_registry` (the live `callisto release execute` publish path) was rewritten independently of the older `PublishOrchestrator`/`SubprocessRegistryClient` pipeline and silently dropped real behavior along the way:
  
  - **npm**: it hardcoded plain `npm publish`, regardless of the workspace's actual package manager. A pnpm or yarn workspace published with the wrong tool -- a live, real bug. It now detects `pnpm-lock.yaml`/`yarn.lock`/`bun.lock(b)` and builds the correct `pnpm publish --filter`/`yarn workspace ... npm publish`/`bun publish`/`npm --workspace` invocation.
  - **cargo**: `cargo publish` ran with no `--locked` and no check that the on-disk manifest version matched the release plan. Both are restored; a mismatch now fails fast with `GraphError::OnDiskVersionDrift` instead of silently publishing a stale version.
  - **pypi**: it built into a throwaway temp directory and uploaded whatever landed there. It now builds into `dist/` and uploads the exact `dist/<normalized-name>-<version>*` glob, matching the older client's intentional scoping.
  - **git tag creation**: it inlined its own `git tag --no-sign -a ...` call with no `--` end-of-options guard against a malicious/malformed tag name. It now goes through `callisto_vcs::GitDataSource::create_tag`, which already has that guard -- extended with a new `TagSignPolicy` parameter (`RespectRepoConfig` for the existing non-durable `callisto tag` path, `ForceUnsigned` for the durable executor) so both safety properties hold together instead of one replacing the other.
  
  The corrected argv construction and output classification for all three registry ecosystems now live in a new pure module, `callisto_graph::commands::registry_argv` (no `CommandRunner`/orchestration logic of its own). `PublishOrchestrator`, `SubprocessRegistryClient`, `AlwaysRetryPolicy`, `SystemTimeProvider`, and `parse_retry_after` are deleted: nothing in production ever constructed them (`callisto publish`'s CLI handler is an explicit dry-run-only compatibility preview; `release execute` is the only real effect path), and their retry/backoff/pre-check policy has no equivalent in the durable executor's model, which persists `Attempting` state and relies on a later re-invocation to retry rather than an in-process retry loop.
- **Delete unused abstractions with zero real callers; share tag-glob compilation**
  
  - `GitVcsProvider` trait and its `GitRepository` impl (`callisto-vcs`) -- fully superseded by the already-polymorphic `GitDataSource` trait (`GitRepository`/`ShellGit`/`GitAccess`, used generically throughout `callisto-graph`); `GitVcsProvider` had exactly one impl and zero callers anywhere in the workspace.
  - `last_tag_for` and its `select_from_tags` helper (`callisto-graph`) -- re-exported from the crate root "for API compatibility" but the only callers were `last_tag_for`'s own unit tests; `TagIndex::build` already resolves tags via `select_from_tags_cached`.
  - Added `callisto_vcs::compile_tag_glob` -- the one real, shared implementation of "compile a tag glob and map a compile failure to `VcsError::InvalidGlob`", now used by both `GitDataSource` backends and by `callisto-graph`'s tag matching instead of three independent copies.
  
  No behavior change for any real caller.
- **Parse GitHub repository and Git tag identity instead of validating ad hoc (audit pattern F)**
  
  `callisto-model` gains two real identity types replacing several independent, inconsistent ad hoc validations of the same concepts:
  
  - `GitHubRepository::parse` replaces three separate `owner/repo` checks: `GitHubAttestationPolicyV1`'s `is_safe_github_repository` (a bare `split_once('/')` with no character-class check at all), `release_pr.rs`'s `validate_repository`/`valid_repo_part` (ASCII alphanumeric plus `-_.` per part), and an entirely unvalidated `format!("{owner}/{repository}")` built from a parsed Git remote URL in `callisto-graph`. Every caller now parses through one charset rule: both `owner` and `repo` must be non-empty, ASCII alphanumeric plus `-`, `_`, `.`, and must not start or end with `-`. `GitHubAttestationPolicyV1::repository`, `GitHubArtifactAttestationV1::repository`, `ArtifactSlotId::new`'s `repository` parameter, `ReleasePrConfigV1::repository`, `ReleasePrSnapshotV2::repository`, and `ReleasePrPullRequestV2::head_repository` all change from `String` to `GitHubRepository`; it serializes and deserializes as its plain `"owner/repo"` string form (matching `ReleasePackageId`'s existing convention), so no wire-format change.
  
    **Behavior change**: this is a real tightening at a security-sensitive validation boundary (`GitHubAttestationPolicyV1` gates the `gh attestation verify --repo <value>` provenance check), not just an internal refactor. A value that previously passed `is_safe_github_repository` can now be rejected -- concretely, embedded whitespace (for example `"owner name/repo"`), a leading or trailing `-` in either part, or more than one `/`.
  
  - `TagName`'s public `String` field is now private. `TagName::parse` rejects a leading `-` (which `git`/`gh` argument parsers read as a flag, even though it is a legal Git ref character) plus anything `git check-ref-format` would reject (control characters, `..`, `@{`, a `.lock`-suffixed or leading-`.` ref component, and Git's other reserved characters). `TagName::new_unchecked` remains as a documented escape hatch for the few call sites (tag-template rendering, last-tag selection) that can prove their input is already constrained to a trusted, non-`-`-leading charset. `callisto-vcs`'s `list_tags` (both the native and shelled-out backends) now silently skips any existing repository tag ref that fails this check instead of returning it uncritically -- an existing Git ref is always legal, but not necessarily safe to hand to a CLI parser as a bare positional.
  
  `callisto-graph`'s `canonical_git_remote` now rejects a `github.com` remote whose derived owner/repository fails `GitHubRepository::parse` (as `GraphError::UnsafeGitRemote`) instead of forwarding an unvalidated string to `dispatch_forge_release`. `callisto-cli`'s `release-pr decide --repository` now rejects a leading/trailing `-` in the owner or repository name that the prior, slightly looser check accepted.

## 0.6.0

- **Add `ShellGit::staged_changes_since` observation**
  
  `ShellGit` gains a `staged_changes_since(base) -> Vec<StagedChangeV1>` observation (parsed from `git diff --cached --raw`, reading worktree bytes for each entry), exposed through `GitAccess`. This is the credential-free input the release action uses to build a `ReleasePrCommitPlanV1` for the managed release PR, part of moving that update off a local `git push` and onto GitHub's forge commit API so the built-in token never needs `.github/workflows/*` write permission.

## 0.5.0

- Released together with the `workspace` fixed group.

## 0.4.1

- Released together with the `workspace` fixed group.

## 0.4.0

- **Git-access layer decoupling, performance, and small correctness fixes**
  
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
- **Security hardening across publish, git, and subprocess handling**
  
  - **Breaking:** a package name starting with `-` is now reported as its own error, instead of being misreported as a path-traversal error.
  - A malicious package name can no longer inject extra flags into the underlying `cargo publish`/`npm publish`/`pypi publish` command.
  - A malicious `publishConfig.registry` URL can no longer redirect an npm publish to an unapproved registry.
  - An absolute or `..`-containing `changesets.dir`/changelog path in `callisto.toml` is now rejected instead of allowing writes outside the workspace.
  - Credentials no longer leak into error messages from failed git or registry-CLI commands.
  - A runaway subprocess can no longer exhaust memory via unbounded output capture, or hang a command indefinitely.
  - A tag name starting with `-` is now rejected, closing a git argument-injection hole.
- Released together with the `workspace` fixed group.

## 0.3.2

### Patch Changes

- Release update

## 0.3.0

### Minor Changes

- Release update

