# callisto-model

## 0.6.0

- **Add `callisto filter-plan` and the primitives it's built on**
  
  New `callisto filter-plan --plan <plan> --report <report>` filters a publish plan down to what a publish report confirms actually succeeded, dropping anything that failed. Lets a release pipeline run `plan-publish` -> `publish` -> `tag`/`gh release create` as separate steps and have the last two operate on what actually shipped, instead of the pre-publish plan.
  
  Built on two new, additive primitives: `PublishPlan::is_empty()`, and `CreatedTag.isFloatingMajor` (distinguishes a floating major-version alias from an immutable per-version release tag in `callisto tag`'s output). Both are backward-compatible — no existing command's behavior changes.
- Fix `release-pr decide` emitting snake_case field names (`pull_request_number`, `expected_branch`, `replacement_branch`) inside its JSON `action` payload instead of camelCase. `#[serde(rename_all = "camelCase")]` on an internally-tagged enum only renames the variant tag, not fields inside variants; the executor script reads `.action.pullRequestNumber` via `jq`, got `null`, and ran `gh pr view null`, breaking every release run past an existing managed PR.
- Add Callisto-owned release PR decisions with forge snapshot verification
- **Update the managed release PR to use GitHub's forge commit API instead of a local Git push**
  
  `ReleasePrSnapshotV1`, `ReleasePrActionV1`, and `ReleasePrDecisionV1` are replaced by schema-version-2 equivalents: `ReleasePrSnapshotV2`, `ReleasePrActionV2`, and `ReleasePrDecisionV2`. `ReleasePrPullRequestV1`'s `workflow_delta_from_base: bool` is replaced by `ReleasePrPullRequestV2`'s `head_commit: CommitSha`. `ReleasePrActionV2` drops the `Supersede` variant entirely -- there is no replacement, and no runtime fallback to it -- in favor of `Noop`, `Create`, and `Update` variants that name a deterministic staging branch. A new `ReleasePrCommitPlanV1` type builds the typed `createCommitOnBranch` payload from a Git index diff, refusing (with new error codes E149-E154) any `.github/workflows/*` path, non-regular-file Git modes, renames/copies/type-changes, and oversized payloads.
  
  This is consumer-facing: on a public GitHub repository, the built-in `GITHUB_TOKEN` cannot write `.github/workflows/*` through either the Git push protocol or `createCommitOnBranch`'s own file changes, which previously forced a SHA-suffixed replacement branch and PR (visible churn) whenever a workflow file drifted on the base branch. The new approach never writes that path at all, so the replacement branch behavior is gone and updates land on one stable branch. A branch already replaced under the old behavior remains a valid, ordinary managed branch and keeps being updated in place.
  
  Removing the v1 `ReleasePr*` types and the `Supersede` variant is a breaking change for any library consumer of `callisto-model`; this ships as a minor bump since the crate is pre-1.0.

## 0.5.0

- Released together with the `workspace` fixed group.

## 0.4.1

- Released together with the `workspace` fixed group.

## 0.4.0

- **New `callisto matrix` command**
  
  - Discovers napi and maturin platform targets from `package.json`/`pyproject.toml`.
  - Builds a per-triple CI table: host runner, cross-compile flag, artifact name.
  - Reports `engines.node`/`requires-python` versions.
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
- **Dependency hygiene**
  
  - Removed an unused AGPL dependency, keeping these permissively-licensed crates clear of any AGPL code.
- **Manifest writes are batched and more reliable**
  
  - **Breaking (custom manifest integrations only):** implementing callisto's `Manifest` trait now requires a `persist()` method to actually write your changes.
  - Re-running a version bump is now safe and won't risk losing a write.
  - Multiple changes to the same manifest file in one run are now applied together instead of risking one overwriting another.
- **npm publish access modeled as a 3-state enum**
  
  - **Breaking:** `plan-publish`'s npm target now reports `access` (`"public"`/`"restricted"`/unset) instead of a `restricted: bool` — update anything parsing that JSON field.
  - An unscoped npm package with `publishConfig.access: "public"` in its `package.json` is no longer silently dropped during publish planning.
- **Release-pipeline / CI Action contract correctness**
  
  - **Breaking:** several `--format json` field names changed or were added (`validate`, `compose-pr-body`, `tag`, `status`, `plan-publish`) — update any scripts parsing this output.
  - Re-tagging a release that already exists no longer reports the wrong commit sha.
  - The official GitHub Action now actually opens a release PR when changesets are pending — a bug made this step unreachable before.
  - GitHub Releases are now correctly marked prerelease for PEP 440 versions too (e.g. `1.2.3a1`), not just SemVer's `-` syntax.
  - Release notes now include the real changelog section instead of nothing.
- **Security hardening across publish, git, and subprocess handling**
  
  - **Breaking:** a package name starting with `-` is now reported as its own error, instead of being misreported as a path-traversal error.
  - A malicious package name can no longer inject extra flags into the underlying `cargo publish`/`npm publish`/`pypi publish` command.
  - A malicious `publishConfig.registry` URL can no longer redirect an npm publish to an unapproved registry.
  - An absolute or `..`-containing `changesets.dir`/changelog path in `callisto.toml` is now rejected instead of allowing writes outside the workspace.
  - Credentials no longer leak into error messages from failed git or registry-CLI commands.
  - A runaway subprocess can no longer exhaust memory via unbounded output capture, or hang a command indefinitely.
  - A tag name starting with `-` is now rejected, closing a git argument-injection hole.

## 0.3.2

### Patch Changes

- Release update

## 0.3.0

### Minor Changes

- Release update

