# callisto-model

## 0.7.1

- Released together with the `workspace` fixed group.

## 0.7.0

- **Fix `ReleaseOperationId::Ord` dropping `attestation_policy` for artifact-upload operations**
  
  `ReleaseOperationId`'s hand-written `Ord` impl folded an `ArtifactUpload` role down to just `platform`/`asset_name`, silently dropping `ArtifactSlotId::attestation_policy` (repository, workflow path, workflow commit) from the comparison -- even though it's part of `Eq`/`Hash`. Two artifact-upload operations for the same package/version/platform/asset_name but a different `attestation_policy` compared `Ord::Equal` while remaining `Eq`-distinct, violating the invariant `BTreeSet`/`BTreeMap` require. `validate_operations`'s duplicate check and `stable_kahn_order`'s prerequisite graph both key on `ReleaseOperationId`, so the second such operation's `BTreeSet` insert silently returned `false` as if it were a real duplicate, and `ReleaseIntentV1::new` wrongly rejected a legitimate, distinct release intent with `DuplicateOperation`.
  
  `ReleaseOperationRole` now derives `PartialOrd`/`Ord` instead of `ReleaseOperationId` hand-decomposing it into a lossy sort key; every variant's field types were already `Ord`, so the derived impl automatically covers every field of every variant (including `attestation_policy`) consistently with the derived `Eq`, and stays correct if a role variant ever gains a field.
- **Delete unused abstractions with zero real callers**
  
  Removed as pure dead-code deletions -- each had exactly one implementation (or none) reachable only through its own tests, with every real call site already going through the underlying concrete type or free function directly:
  
  - `PackageIdentityResolver` trait (`callisto-model`) -- a one-line delegate to `PackageId::matches`.
  - `ChangesetStorage` trait (`callisto-model`, re-exported from `callisto-manifests`) -- a one-line method-call spelling of the free function `atomic_write`.
  - `ManifestCstEditor` trait and its blanket impl (`callisto-manifests`) -- combined mutate-and-persist in one call, which no real caller used and which would have been the wrong shape anyway: `apply.rs` deliberately calls `write_version`/`update_dependency_spec` and `persist` as separate steps to batch several writes before one flush.
  - `ReleaseEffectAdapter` trait and `PreparedReleaseEffectAdapter` (`callisto-graph`) -- `execute_release`/`execute_release_with_artifacts` now call the prepared capability's dispatch directly instead of going through a single-implementation generic seam.
  - `RawMoonYml`/`RawMoonExtensions`/`RawMoonCallistoConfig` (`callisto-graph`) -- a config schema nothing in this crate ever deserialized a `moon.yml` file into.
  - `ReportPresenter` trait (`callisto-cli`) -- only implemented by a test-only fake built solely to exercise the trait's own default method.
  - `--skip-publish-precheck` CLI flag and `check_credentials` (`callisto-cli`) -- the flag was parsed and immediately discarded; `check_credentials` is `#[cfg(test)]`-gated and was never compiled into the production binary.
  
  No behavior change for any real caller.
- **Fix `fixed_group_target` reading a stale group member's (missing) slot in `base`, corrupting the shared alignment version**
  
  Track 1 (Fixed-group convergence) in `solve_cascade` passed the raw `GroupDef` into `groups::fixed_group_target`, which iterated every declared member -- not just the ones still present in `input.base` -- when picking the group's alignment base. A group member removed from the workspace but still declared in `callisto.toml`, if it happened to carry a release tag from before its removal, could be picked as `released[0]`; `base.get(&released[0])` then missed (the stale id has no entry in `base`) and silently fell back to the hardcoded `Version::semver(1, 0, 0)` default, corrupting the group's shared target version for every live sibling. `fixed_group_target` now takes an already-filtered `live_members` slice instead of the raw `GroupDef`, mirroring the stale-member guard the Linked-group block directly above it already applies -- a stale member can no longer occupy the alignment slot regardless of declaration order or tag history.
  
  **Fix `ReleaseOperationId`/`ArtifactSlotId`'s `Ord` comparing a `Version` field via `render()` instead of `(grammar, raw)`**
  
  Residual instance of the same defect class as the earlier `ReleaseOperationRole` fix (which stopped `Ord` from dropping `attestation_policy`): both hand-written `Ord` impls keyed their `version: Version` field on `version.render()` alone, dropping `version.grammar()`. The identical literal string parses under more than one grammar (`"1.2.3"` is valid both as SemVer and PEP 440), so two otherwise-identical `ReleaseOperationId`/`ArtifactSlotId` values that are `Eq`-distinct only by grammar compared `Ordering::Equal`, violating the invariant `BTreeSet`/`BTreeMap` require: `a.cmp(b) == Equal` iff `a == b`. Both now key the version field on `(grammar, raw)` via a shared `version_ord_key` helper.
- `PackageId::parse`'s `:`-prefixed and bare-with-slash branches independently ran the same three-check validation sequence on the post-separator remainder (empty-after-prefix, leading-hyphen, path-traversal) before constructing an identical `PackageId::Prefixed`. The `LeadingHyphen` check had already been hand-added to both copies separately in one prior commit.
  
  Extracted `PackageId::validate_prefixed_remainder` as the single implementation, called from both branches. No behavior change -- error variants, field values, and messages are unchanged for every existing case.
- What each `PublishTarget` variant means -- whether it's dispatchable, how it contributes to a package's semantic fingerprint, and whether it carries an explicit registry override -- was independently re-encoded in three separate match statements (`callisto-graph`'s `target_fingerprint`, `prepared_registry_binding`, and `publish.rs`'s per-target dispatch loop), kept in sync only by convention with `callisto-model`'s existing `PublishTarget`/`Ecosystem` capability layer.
  
  Extended that existing layer instead of adding a second one: `PublishTarget` gains `registry_override()` (the `Npm.registry`/`Pypi.index`/`NuGet.source` payload field, unified) and `is_implemented()`. `PublishTarget` and `Ecosystem` are not 1:1 -- `GitHubRelease` is a VCS release action with no backing package `Ecosystem`, and `None` is the "not configured to publish" sentinel -- so `is_implemented()` special-cases both (`GitHubRelease` is always unimplemented, `None` is vacuously implemented) and otherwise defers to `Ecosystem::is_implemented()`. `target_fingerprint` now reuses the existing `config_str()` instead of re-hardcoding its own kind strings; `prepared_registry_binding` now calls `registry_override()` instead of its own separate match; `publish.rs`'s dispatch loop now decides its `PublishTargetNotImplemented` diagnostic via `is_implemented()` instead of independently naming `NuGet`/`GitHubRelease`. Pure refactor -- no behavior change for any existing test case.
- Replace `derive_release_commit_decision`'s 2*N per-manifest `git show` spawns with 2 batched `git cat-file --batch` invocations.
  
  Verifying a merged release commit's claimed version roster looped over every workspace package's canonical manifests and, for each one, called `manifest_version_at` twice -- once against the parent commit, once against the release commit -- each shelling out a separate `git show <commit>:<path>` subprocess. For a monorepo with N canonical manifests that was 2N sequential subprocess spawns to verify one release commit, on top of the `git diff-tree` call already made earlier in the same function.
  
  Since only two distinct commits are ever queried, both are now fetched with exactly one `git cat-file --batch` invocation each: every requested manifest path is written to the child's stdin up front, and each object's blob content (or `missing`, for a manifest that didn't exist yet at the parent commit) streams back on stdout in one round trip. `CommandRunner` gains a new `run_with_stdin` method (default: `Unsupported`, so none of the many existing test-double/`moon` implementors need to change) that `CliCommandRunner` implements for real, piping input on a dedicated writer thread concurrently with draining the child's stdout/stderr so a payload larger than a pipe buffer can't deadlock.
  
  Because this feeds release-commit trust verification, the batch-output parser is deliberately defensive rather than permissive: each response is matched against its exact requested `commit:path` object string (not a generic pattern), a declared content length that doesn't fit the remaining output or isn't followed by the protocol's separator byte fails the whole batch closed, and any leftover or truncated output once every requested path is accounted for is rejected rather than ignored. A new regression test drives `derive_release_commit_decision` with a recording `CommandRunner` double across three canonical manifests and asserts exactly 2 `cat-file --batch` invocations occur, not 6.
- `ReleaseDecisionV1::new` now rejects a decision that claims divergent target versions for two entries tagged with the same `FixedGroup`/`LinkedGroup` inclusion reason. A fixed or linked group always converges every member to one shared target version; a decision claiming otherwise is malformed, whether freshly derived from a version plan or read back from a committed release-decision file (the digest-validating `Deserialize` impl calls `new()` internally, so this check applies to both). The check is self-contained: it reads only the decision's own entries and their self-declared group tags, with no workspace/`GroupTable` access needed.
- `release.rs` and `release_pr.rs` each hand-wrote the same wire schema-version gate independently: deserialize into a private `Wire` struct, compare `wire.schema_version` against `Self::SCHEMA_VERSION`, and return a hand-written "unsupported ... schema version" `serde::de::Error::custom(...)` on mismatch. Six sites did this separately -- `ReleaseDecisionV1`, `ReleaseIntentV1` (which also inlines the same check for its nested `decision` and `snapshot` schema versions) in `release.rs`, and `ReleasePrConfigV1`, `ReleasePrSnapshotV2`, `ReleasePrDecisionV2`, `ReleasePrCommitPlanV1` in `release_pr.rs`.
  
  Extracted `check_schema_version` as the single implementation, called from all six `Deserialize` impls. No behavior change -- each site's exact error wording is preserved verbatim (the six messages already differed from each other by type name, e.g. "unsupported release decision schema version" vs. "unsupported release PR commit plan schema version", so each is now produced by passing that same descriptive name into the shared helper rather than standardized to one wording).
- **Add a shared canonical-manifest-identity reader; stop hand-parsing Cargo.toml/package.json/pyproject.toml independently in six places**
  
  `callisto-model` gains `Ecosystem::CANONICAL` (`[Cargo, Npm, Pypi]`) and `Ecosystem::canonical_manifest_format()`, the single enumeration of "which ecosystems have a canonical identity manifest, and which format is it" -- replacing hand-written `if root.join("Cargo.toml").exists() { ... } else if ...` chains.
  
  `callisto-manifests` gains `read_identity(format, source, path) -> Result<ManifestIdentity, ManifestError>`, a pure, I/O-free reader for callers that already hold a manifest's content as a string (a `git show` blob, a directory walker's pre-read buffer) instead of a path `Manifest::open` can read from disk. `ManifestIdentity { name, version }` carries `version` as a new `VersionSource` enum (`Literal(String)` vs `InheritedFromWorkspace`) rather than collapsing Cargo's `version.workspace = true` into `None`. Also adds `read_napi_targets(path, &Value) -> Result<Option<Vec<String>>, ManifestError>`, the one shared parser for `napi.targets`.
  
  Six call sites in `callisto-graph`/`callisto-moon` now route through these instead of re-implementing the same extraction: `IdentityResolver::resolve`, `IgnoreWalkLocator::projects`, `MoonProjectLocator`'s ecosystem detection, workspace-membership manifest-filename lookups, `NapiTargetsIndex::load`, and `matrix::read_napi_targets`.
  
  Two real behavior changes fall out of this:
  
  - **`IgnoreWalkLocator::projects()` now discovers Flit-based Python packages.** Its own pyproject.toml parsing previously only checked `project.name` and `tool.poetry.name`, missing the `tool.flit.metadata.module` fallback the shared extractor already had -- a Flit package was silently undiscovered before this change.
  - **`NapiTargetsIndex::load` and `matrix::read_napi_targets` now share one parser with their policy difference visible at the call site**: both still disagree on how to handle a malformed `napi.targets` (`NapiTargetsIndex::load` stays lenient via `.ok()`, `matrix::read_napi_targets` stays strict by propagating the error), but a `napi.targets` array containing a non-string entry now causes `NapiTargetsIndex::load` to drop the whole array instead of silently filtering out just the bad entry -- a narrower form of the same lenient policy, no longer a second independent implementation.
  
  `GraphError`'s `manifest_version_at` (release-commit verification against a historical git blob) also now goes through `read_identity` instead of its own inline `toml_edit`/`serde_json` parsing.
- `GroupName` and `RegistryKey` (`identity.rs`) each hand-wrote the identical shape: same derive list, `#[schemars(with = "String")]`, `#[serde(transparent)]`, an `as_str(&self) -> &str { &self.0 }`, and a `Display` impl that just wrote the inner string. Extracted a `string_newtype!` macro that expands to that whole shape from a name (plus an inner `string_newtype_display!` for just the `as_str`/`Display` pair); both types now expand from one invocation each. `RegistryKey`'s well-known-key associated consts stay in their own hand-written `impl` block, unaffected.
  
  `TagName` (`tag.rs`) looked identical at a glance but isn't: it validates on construction/deserialize (private field, `parse`/`new_unchecked`, hand-written `Serialize`/`Deserialize` routed through `parse`), the same intentionally-different shape as `CommitSha`. Only its duplicated `as_str`/`Display` pair -- genuinely identical text to the other two -- was switched to `string_newtype_display!`; its struct definition, derives, and validating serde impls are untouched. No behavior change: public API, trait impls, and serde wire format are identical before and after for all three types.
- **Parse GitHub repository and Git tag identity instead of validating ad hoc (audit pattern F)**
  
  `callisto-model` gains two real identity types replacing several independent, inconsistent ad hoc validations of the same concepts:
  
  - `GitHubRepository::parse` replaces three separate `owner/repo` checks: `GitHubAttestationPolicyV1`'s `is_safe_github_repository` (a bare `split_once('/')` with no character-class check at all), `release_pr.rs`'s `validate_repository`/`valid_repo_part` (ASCII alphanumeric plus `-_.` per part), and an entirely unvalidated `format!("{owner}/{repository}")` built from a parsed Git remote URL in `callisto-graph`. Every caller now parses through one charset rule: both `owner` and `repo` must be non-empty, ASCII alphanumeric plus `-`, `_`, `.`, and must not start or end with `-`. `GitHubAttestationPolicyV1::repository`, `GitHubArtifactAttestationV1::repository`, `ArtifactSlotId::new`'s `repository` parameter, `ReleasePrConfigV1::repository`, `ReleasePrSnapshotV2::repository`, and `ReleasePrPullRequestV2::head_repository` all change from `String` to `GitHubRepository`; it serializes and deserializes as its plain `"owner/repo"` string form (matching `ReleasePackageId`'s existing convention), so no wire-format change.
  
    **Behavior change**: this is a real tightening at a security-sensitive validation boundary (`GitHubAttestationPolicyV1` gates the `gh attestation verify --repo <value>` provenance check), not just an internal refactor. A value that previously passed `is_safe_github_repository` can now be rejected -- concretely, embedded whitespace (for example `"owner name/repo"`), a leading or trailing `-` in either part, or more than one `/`.
  
  - `TagName`'s public `String` field is now private. `TagName::parse` rejects a leading `-` (which `git`/`gh` argument parsers read as a flag, even though it is a legal Git ref character) plus anything `git check-ref-format` would reject (control characters, `..`, `@{`, a `.lock`-suffixed or leading-`.` ref component, and Git's other reserved characters). `TagName::new_unchecked` remains as a documented escape hatch for the few call sites (tag-template rendering, last-tag selection) that can prove their input is already constrained to a trusted, non-`-`-leading charset. `callisto-vcs`'s `list_tags` (both the native and shelled-out backends) now silently skips any existing repository tag ref that fails this check instead of returning it uncritically -- an existing Git ref is always legal, but not necessarily safe to hand to a CLI parser as a bare positional.
  
  `callisto-graph`'s `canonical_git_remote` now rejects a `github.com` remote whose derived owner/repository fails `GitHubRepository::parse` (as `GraphError::UnsafeGitRemote`) instead of forwarding an unvalidated string to `dispatch_forge_release`. `callisto-cli`'s `release-pr decide --repository` now rejects a leading/trailing `-` in the owner or repository name that the prior, slightly looser check accepted.

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

