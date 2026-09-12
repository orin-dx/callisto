# callisto-cli

## 0.7.1

- Released together with the `workspace` fixed group.

## 0.7.0

- **Fix three error/diagnostic messages that told the operator the wrong cause**
  
  - `callisto-vcs`: `ReleaseWorkspaceLock::acquire` reported every lock-acquisition failure -- permission denied, disk full, a filesystem that doesn't support `flock`, or any other I/O error -- as "another Callisto release already holds this workspace lock". It now distinguishes true contention (matched against `fs2::lock_contended_error()`'s `ErrorKind`, the same signal `fs2` itself uses) from other I/O failures, and reports the real underlying cause for the latter instead of misattributing it to a held lock.
  - `callisto-graph`: `IdentityResolver::resolve` collapsed both an unsupported-ecosystem case and a genuine `PackageId` parse failure into `GraphError::AmbiguousName`, which is semantically wrong for both (neither is a name-ambiguity problem) and discarded the real reason. Two new variants -- `UnsupportedIdentityEcosystem` and `PackageIdentifierParse` (carrying the underlying `PackageIdParseError`) -- now report each failure's own accurate cause.
  - `callisto-cli`: `callisto add --package name:severity` rejected an invalid severity with "Must be patch, minor, or major.", omitting `none`, which `Severity::from_str` has always accepted. The message (and the `--package` flag's help text) now lists all four accepted values.
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
- Four instances of a value being re-derived instead of reused, all fixed the same way (compute once, have both branches consume the same value):
  
  - `crates/callisto-cli/src/commands/status.rs`'s `--check` exit code recomputed `has_changesets` from `report.packages` instead of reading the field `callisto-graph`'s status command already computed and stored on `StatusReport`.
  - `crates/callisto-graph/src/commands/validate.rs`'s `--staged` and `--since` branches each ran their own `git diff`, error-mapped it, and filtered changesets identically, differing only in the git args. Factored into one `changesets_touched_by` helper.
  - `crates/callisto-graph/src/commands/tag.rs`'s floating-major tag computation (template lookup, version extract, grammar resolve, parse, render, existence check) was written out fully in both the dry-run preview branch and the write branch. Factored into `plan_floating_major`, called by both; the permit still gates only the actual `create_floating_major` call.
  - `crates/callisto-cli/src/commands/publish.rs` and `plan_publish.rs` independently wired up the identical `load_workspace` -> `plan_publish` pipeline. Factored into a shared `plan_publish::build_plan` both commands call.
- **Restructure the release PR body to group by change instead of by package, and drop redundant/misleading sections**
  
  A single `.changeset/*.md` file can name several packages, and its summary was copied verbatim into every one of them (correct and desirable for each package's own standalone `CHANGELOG.md`) -- but the PR body then rendered that same text once per package it touched, producing bodies with the same paragraphs repeated multiple times for any release spanning a fixed/linked group or a multi-package changeset. A real 10-package release PR measured at 629 lines.
  
  - `compose-pr-body`'s `### 📦 What's Changing` section (renamed from `Package Release Details`) now groups by the underlying change (a changeset file, a commit, a dependency/peer bump) and lists every package it affects in one block, instead of one full block per package. A package bumped only via a fixed/linked group with no direct change of its own is now named in a short note instead of getting an empty-looking collapsible.
  - The whole section is wrapped in one collapsible, closed by default and open only when the release contains a major bump; each change's own block follows the same rule. A routine minor/patch release now renders as just the summary table, one note, and a single closed toggle.
  - The redundant "#### Version Change" sub-heading is gone (already shown one line above in each block's `<summary>`), and the major-bump callout uses `[!WARNING]` instead of `[!IMPORTANT]`.
  - The summary table's Bump column now includes a severity emoji (🔴/🟡/🟢) for at-a-glance scanning.
  - The "Suggested PR Labels" line is removed: the label it named was never merely suggested (the release-PR script always applies it via `gh pr create --label`/`gh pr edit --add-label`), so the line was both redundant with GitHub's own label UI and factually misleading. `compose-pr-body`'s now-pointless `--label` flag is removed along with it (`ComposePrBodyArgs`/`PrBodyOptions` drop the `labels` field); the release-PR action script no longer passes `--label` to `compose-pr-body`, only to the real `gh pr create`/`gh pr edit` calls where it's actually applied.
  
  No changes to changeset authoring, parsing, or per-package `CHANGELOG.md` output -- this is scoped entirely to the PR body, the one place the same fanned-out content is read together in a single sitting.
- Two `callisto-cli` sites re-derived `PublishPlan` emptiness by hand instead of calling the canonical `PublishPlan::is_empty()` (`callisto-model`), which already ANDs all five plan fields specifically to avoid a hand-listed check silently missing one (e.g. `pypi_packages`):
  
  - `commands/publish.rs`'s `write_dry_run_text` hand-listed all five fields with `&&`.
  - `render/mod.rs`'s `render_publish` computed a `total_packages` sum of the four package-field lengths plus a separate `releases.is_empty()` check.
  
  Both now call `plan.is_empty()`. No behavior change -- `render_publish` keeps `total_packages` for its package-section-skip branch and package counts, only the true-emptiness branch now calls `is_empty()`.
- Replace `derive_release_commit_decision`'s 2*N per-manifest `git show` spawns with 2 batched `git cat-file --batch` invocations.
  
  Verifying a merged release commit's claimed version roster looped over every workspace package's canonical manifests and, for each one, called `manifest_version_at` twice -- once against the parent commit, once against the release commit -- each shelling out a separate `git show <commit>:<path>` subprocess. For a monorepo with N canonical manifests that was 2N sequential subprocess spawns to verify one release commit, on top of the `git diff-tree` call already made earlier in the same function.
  
  Since only two distinct commits are ever queried, both are now fetched with exactly one `git cat-file --batch` invocation each: every requested manifest path is written to the child's stdin up front, and each object's blob content (or `missing`, for a manifest that didn't exist yet at the parent commit) streams back on stdout in one round trip. `CommandRunner` gains a new `run_with_stdin` method (default: `Unsupported`, so none of the many existing test-double/`moon` implementors need to change) that `CliCommandRunner` implements for real, piping input on a dedicated writer thread concurrently with draining the child's stdout/stderr so a payload larger than a pipe buffer can't deadlock.
  
  Because this feeds release-commit trust verification, the batch-output parser is deliberately defensive rather than permissive: each response is matched against its exact requested `commit:path` object string (not a generic pattern), a declared content length that doesn't fit the remaining output or isn't followed by the protocol's separator byte fails the whole batch closed, and any leftover or truncated output once every requested path is accounted for is rejected rather than ignored. A new regression test drives `derive_release_commit_decision` with a recording `CommandRunner` double across three canonical manifests and asserts exactly 2 `cat-file --batch` invocations occur, not 6.
- Replace release-commit verification's changeset re-derivation with persist-and-verify. `callisto version --emit-decision <path>` now writes the exact release decision (package, target version, inclusion reason) it computed, alongside the manifest and changelog edits it already writes. `callisto release plan --from-release-commit` (now requiring a companion `--decision <path>`) verifies the merged commit's actual diff against that committed decision instead of re-deriving changeset, fixed-group, linked-group, cascade, and pre-release-policy inclusion from raw git history a second time.
  
  The prior re-derivation only understood a direct changeset-to-package match: it rejected any real release where a fixed-group cascade bumped a sibling package the deleted changeset never named, which is every release in a fixed-group workspace once more than one package versions together. Trusting the already-computed decision, tamper-checked via its own content digest, removes that entire reimplementation and the drift it was exposed to.
- `snapshot` and `tag` now share a new `abort_on_crosscheck_failures` helper (`crates/callisto-cli/src/commands/mod.rs`) instead of each hand-rolling an identical ~15-line block that cloned `ws.graph.diagnostics()`, called `callisto_graph::commands::escalate`, filtered for `Error` severity, and formatted the same abort message.
  
  **Fix**: that duplicated block called `escalate(&mut diags, true, true)` -- hardcoding both the `strict` and `strict_graph` arguments to `true` -- instead of passing through the command's actual flags, the way `status`, `validate`, and `version` already do via their `*Options` structs. In practice this meant `--strict-graph` had no CLI flag at all on `snapshot`/`tag` and no way to take effect on its own: the escalation block only ran from behind an `if args.strict` gate, so a workspace-graph-only strict check silently did nothing unless `--strict` was also passed. Both commands now accept a real `--strict-graph` flag (`SnapshotArgs`/`TagArgs` gained a `strict_graph: bool` field) and pass `args.strict`/`args.strict_graph` through to `escalate` unconditionally, matching the sibling commands' semantics: `--strict` still escalates graph diagnostics too (per `escalate`'s own `strict || strict_graph` rule), and `--strict-graph` alone now escalates them as well.
- **Parse GitHub repository and Git tag identity instead of validating ad hoc (audit pattern F)**
  
  `callisto-model` gains two real identity types replacing several independent, inconsistent ad hoc validations of the same concepts:
  
  - `GitHubRepository::parse` replaces three separate `owner/repo` checks: `GitHubAttestationPolicyV1`'s `is_safe_github_repository` (a bare `split_once('/')` with no character-class check at all), `release_pr.rs`'s `validate_repository`/`valid_repo_part` (ASCII alphanumeric plus `-_.` per part), and an entirely unvalidated `format!("{owner}/{repository}")` built from a parsed Git remote URL in `callisto-graph`. Every caller now parses through one charset rule: both `owner` and `repo` must be non-empty, ASCII alphanumeric plus `-`, `_`, `.`, and must not start or end with `-`. `GitHubAttestationPolicyV1::repository`, `GitHubArtifactAttestationV1::repository`, `ArtifactSlotId::new`'s `repository` parameter, `ReleasePrConfigV1::repository`, `ReleasePrSnapshotV2::repository`, and `ReleasePrPullRequestV2::head_repository` all change from `String` to `GitHubRepository`; it serializes and deserializes as its plain `"owner/repo"` string form (matching `ReleasePackageId`'s existing convention), so no wire-format change.
  
    **Behavior change**: this is a real tightening at a security-sensitive validation boundary (`GitHubAttestationPolicyV1` gates the `gh attestation verify --repo <value>` provenance check), not just an internal refactor. A value that previously passed `is_safe_github_repository` can now be rejected -- concretely, embedded whitespace (for example `"owner name/repo"`), a leading or trailing `-` in either part, or more than one `/`.
  
  - `TagName`'s public `String` field is now private. `TagName::parse` rejects a leading `-` (which `git`/`gh` argument parsers read as a flag, even though it is a legal Git ref character) plus anything `git check-ref-format` would reject (control characters, `..`, `@{`, a `.lock`-suffixed or leading-`.` ref component, and Git's other reserved characters). `TagName::new_unchecked` remains as a documented escape hatch for the few call sites (tag-template rendering, last-tag selection) that can prove their input is already constrained to a trusted, non-`-`-leading charset. `callisto-vcs`'s `list_tags` (both the native and shelled-out backends) now silently skips any existing repository tag ref that fails this check instead of returning it uncritically -- an existing Git ref is always legal, but not necessarily safe to hand to a CLI parser as a bare positional.
  
  `callisto-graph`'s `canonical_git_remote` now rejects a `github.com` remote whose derived owner/repository fails `GitHubRepository::parse` (as `GraphError::UnsafeGitRemote`) instead of forwarding an unvalidated string to `dispatch_forge_release`. `callisto-cli`'s `release-pr decide --repository` now rejects a leading/trailing `-` in the owner or repository name that the prior, slightly looser check accepted.
- **Render the `governed by <key> = <value> (default)` attribution line for bumps and diagnostics (§13 invariant 28)**
  
  `render::attribution::attribution_line` has existed since the config-provenance work landed but had zero callers: `render_version`'s text output printed a bump's package/from/to and a diagnostic's severity/message, but never consulted `BumpRecord.governed_by` / `Diagnostic.governed_by` at all, so an operator running `callisto version` (or `--format text` generally) had no way to see which `callisto.toml` key was responsible for a bump or warning, or whether that key was set explicitly or left at its default.
  
  `render_version` now takes the workspace's `&ResolvedConfig` and prints the attribution line under any bump or diagnostic whose `governed_by` is `Some`, via the same `attribution_line` the spec always intended to render it (`callisto-graph` computes the key, `callisto-cli` looks up and renders its current value and provenance -- neither crate re-derives the other's half). `render_diagnostics` gained a matching `Option<&ResolvedConfig>` parameter so it can do the same for any report's diagnostics; every caller other than `render_version` passes `None` today, since no other report ever populates `governed_by` on a `Diagnostic` yet, so their output is unchanged.
  
  **Signature change**: `render::render_version(report, w)` is now `render::render_version(report, cfg, w)`. `render::render_diagnostics(diagnostics, w)` is now `render::render_diagnostics(diagnostics, cfg, w)`. Both are `callisto-cli`'s own rendering functions with one production call site each (`commands/version.rs`); no other crate calls them today.
  
  Not included: `Diagnostic.governed_by` attribution for `StatusReport`, `PublishReport`, `PublishPlan`, `ValidateReport`, and `MatrixReport` is still a no-op, because none of those report kinds ever construct a diagnostic with `governed_by: Some(_)` today -- wiring it up now would mean threading `ResolvedConfig` through several more CLI command handlers for a code path with nothing to render yet. Tracked as a follow-up for whenever one of those diagnostics starts carrying a real `governed_by`, rather than done speculatively here.

## 0.6.0

- **Add `callisto filter-plan` and the primitives it's built on**
  
  New `callisto filter-plan --plan <plan> --report <report>` filters a publish plan down to what a publish report confirms actually succeeded, dropping anything that failed. Lets a release pipeline run `plan-publish` -> `publish` -> `tag`/`gh release create` as separate steps and have the last two operate on what actually shipped, instead of the pre-publish plan.
  
  Built on two new, additive primitives: `PublishPlan::is_empty()`, and `CreatedTag.isFloatingMajor` (distinguishes a floating major-version alias from an immutable per-version release tag in `callisto tag`'s output). Both are backward-compatible — no existing command's behavior changes.
- Add Callisto-owned release PR decisions with forge snapshot verification
- **Fix: credential redaction now covers the live-streamed CI log, not just the captured error**
  
  `callisto publish` streams a registry command's stderr to the terminal in real time as it runs, separately from the captured copy redacted afterward for the final error message. A credential embedded in that stderr (e.g. a private registry URL with basic auth) was previously redacted only in the captured copy -- the live stream, which a CI log persists, was not. Both are now redacted identically; the captured copy stays raw internally so error classification (rate-limit, auth-failure detection) still works on the exact upstream text.
- **Add `callisto release-pr commit-plan`**
  
  A new read-only `callisto release-pr commit-plan --base-commit <sha> --message <msg> [--out <file>]` subcommand renders a `ReleasePrCommitPlanV1` as JSON from the current Git index diff against `<sha>`. It is the building block the release action now uses to stage a release-PR update through GitHub's `createCommitOnBranch` commit API instead of a local `git push`, so the built-in `GITHUB_TOKEN` never needs `.github/workflows/*` write permission on any ref. This removes the SHA-suffixed-replacement-branch churn the prior local-push fallback caused whenever a workflow file changed on the base branch.
- **Add `--skip-publish-precheck` to skip the redundant already-published registry check**
  
  `callisto publish` previously called the registry's `is_published` check before every publish attempt, even though a fresh publish always returns false there and a conflicting publish is already correctly classified as `AlreadyPublished` from the publish call itself. Pass `--skip-publish-precheck` to skip that extra round-trip; the default behavior is unchanged.

## 0.5.0

- # Native artifact placement for CI-built platform binaries
  
  - The shipped GitHub Action (`callisto-action`) now downloads each `callisto matrix`-driven native build's CI artifact and places it into its package directory automatically before `callisto publish` runs -- closing the gap between the build matrix and a working end-to-end release for napi-rs/maturin platform packages. No consumer-authored placement step is needed; it happens whenever `nativeMatrix` is non-empty and `publish` is enabled.
  - Fixed: two packages sharing the same target platform triple no longer collapse into one `artifactName`, which previously dropped one package's build from the release silently.
  - `callisto matrix --format json`'s `artifactName` values now follow napi-rs's own `<name>-<platform>-<arch>[-<abi>]` convention (previously the raw Rust target triple), and scoped npm package names such as `@scope/addon` no longer produce an `artifactName` containing `/`, which `actions/upload-artifact` rejects.

## 0.4.1

- Released together with the `workspace` fixed group.

## 0.4.0

- **New `callisto matrix` command**
  
  - Discovers napi and maturin platform targets from `package.json`/`pyproject.toml`.
  - Builds a per-triple CI table: host runner, cross-compile flag, artifact name.
  - Reports `engines.node`/`requires-python` versions.
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

## 0.3.3

### Patch Changes

- # Security hardening, correctness fixes, and performance improvements
  
  ## Security
  
  - **Argument injection in publish client** (`callisto-graph`): Package names are now validated before being passed to `cargo publish`, `npm publish`, and `pypi publish`. Names beginning with `-` are rejected with an error, preventing a crafted package name from injecting flags into the subprocess.
  - **Path traversal in `changesets.dir` config** (`callisto-graph`): The `changesets.dir` value from `callisto.toml` is now validated to contain no `..` components. A value like `../../tmp` previously allowed all changeset read and write operations to escape the workspace root.
  
  ## Correctness
  
  - **Changelog idempotency on stable release after pre-release** (`callisto-changelog`): The duplicate-entry guard in `prepend()` used a substring check that incorrectly matched `## 1.0.0-alpha.1` when writing `## 1.0.0`, permanently suppressing the stable release section. Now uses an exact line-boundary match.
  - **Empty changelog section headings** (`callisto-changelog`): `### Patch Changes` (and equivalent headings) are no longer emitted when every entry in the section has a blank summary.
  - **`pre-major-inference` config field now applied** (`callisto-graph`): The per-package `pre-major-inference` setting in `callisto.toml` was parsed and stored but never consulted. `aggregate()` hardcoded `OFF` for every package. The configured policy is now applied when constructing the inference window.
  - **Inference errors now surfaced as diagnostics** (`callisto-graph`): When `SeverityInference::infer` returns an error (e.g. due to a git failure), the error was previously swallowed with no diagnostic emitted and no bump recorded. The affected package now receives a warning-level diagnostic describing the failure.
  - **Ambiguous bare package name now errors in `status`** (`callisto-graph`): In a polyglot workspace containing both `cargo/foo` and `npm/foo`, a changeset entry naming bare `foo` was silently applied to every matching package. `status()` now returns `GraphError::AmbiguousName` with the full candidate list.
  - **Version overflow returns error instead of wrapping** (`callisto-format`): Incrementing a version component equal to `u64::MAX` previously wrapped to `0` in release builds (producing a version lower than the input) and panicked in debug builds. All bump paths now use `checked_add(1)` and return `BumpError::Overflow`.
  - **Decor preserved on full-table dependency form** (`callisto-manifests`): `CargoToml::update_dependency_spec` did not clone and reapply surrounding decor when the dependency used the `[dependencies.name]` full-table form, causing trailing comments and formatting to be lost on the first version bump.
  
  ## Performance
  
  - **O(N²) package lookups eliminated in publish and version commands** (`callisto-graph`): `plan_publish` and `plan_version` called `packages().find()` inside loops over package IDs and severity entries, giving O(N²) complexity. A single `HashMap<&PackageId, &Package>` is now built once before each loop.
  - **O(N) toposort validation** (`callisto-graph`): The toposort subset-validation loop called `all_packages.contains(id)` (O(N) slice scan) per member. A `HashSet` built once before the loop reduces this to O(N) total.
  - **Eliminated intermediate allocations in graph resolver** (`callisto-graph`): `dependencies_of()` and `dependents_of()` allocated a `Vec<&DepEdge>` and immediately called `into_iter()`. Both now return the iterator directly via `flat_map`.
  - **Unique changeset slugs without clock dependency** (`callisto-cli`): `generate_human_slug` derived all three word indices from a single nanosecond timestamp, yielding at most 8,000 distinct values. Concurrent `callisto add` calls on systems with coarse clock resolution could produce the same slug, causing `atomic_write` to silently overwrite the earlier changeset. Slugs now incorporate a process-global atomic counter for guaranteed uniqueness.
  
  ## Housekeeping
  
  - Removed unreachable `calculate_bump_severity` function from `cascade.rs` that was suppressed with `#[allow(dead_code)]`.

## 0.3.2

### Patch Changes

- Release update

## 0.3.0

### Minor Changes

- Release update

## 0.2.0

### Minor Changes

- Release update

