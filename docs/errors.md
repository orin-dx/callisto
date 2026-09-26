# Error codes

Hand-maintained from the `#[diagnostic(code(...))]` attributes in `crates/*/src`. A `callisto-model` test fails when a numeric code is missing here.

186 codes exist across two namespaces: 153 numeric `E###` codes (`callisto-model`, `callisto-format`, `callisto-vcs`, `callisto-changelog`, `callisto-graph`) and 33 `callisto::snake_case` codes (`callisto-cli`'s own top-level errors — the ones the CLI surface actually raises). `callisto-conventional` errors carry no diagnostic code at all. `callisto-cli`'s `CliError` also has 9 variants that wrap another crate's error transparently (`#[diagnostic(transparent)]`, no `code(...)` of its own) — those are intentionally out of scope here since they have no code to document; the code you see for them at runtime is whichever code above their wrapped error already carries.

"Fix" is the code's `help(...)` text verbatim. "—" means the variant has no help text in the source.

## callisto-model

| Code | Meaning | Fix |
| --- | --- | --- |
| E001 | Path is absolute; `callisto-model` paths are workspace-root-relative | Use a relative path relative to the workspace root. |
| E002 | Path is not valid UTF-8; callisto serializes paths into its JSON contract | Ensure all workspace file paths contain valid UTF-8 characters. |
| E003 | Path attempts to traverse outside the workspace root | Keep workspace file paths strictly within the workspace directory. |
| E004 | Value is not a valid 40-character hexadecimal commit SHA | Provide a valid full 40-character commit SHA. |
| E005 | Manifest role is not valid for the manifest's format | — |
| E006 | Package has no canonical manifest; at least one is required | Add a canonical manifest file for the package. |
| E007 | Package's canonical manifests disagree on version grammar; it has no single version of record | Ensure all manifests for a package use consistent version grammars. |
| E008 | Package identity has an ecosystem prefix but no name after it | Include package name after ecosystem prefix. |
| E009 | Manifest path has an unsupported format | — |
| E010 | Failed to read a manifest file | Check file permissions and path validity. |
| E011 | Failed to write a manifest file | Check write permissions on target directory. |
| E012 | Manifest is not valid for its declared format | Verify manifest syntax formatting. |
| E013 | Manifest is missing a required field | — |
| E014 | Manifest's declared version string is invalid | Fix version string to follow valid semver or ecosystem grammar. |
| E015 | Manifest inherits a key from the workspace root; that key must be written on the root manifest instead | Update workspace inheritance key in root manifest. |
| E016 | Manifest format is not a supported write target | — |
| E017 | Manifest has no dependency of the given kind and name | — |
| E018 | Operation is not supported for this manifest's format | — |
| E019 | Format-preserving write would not round-trip | Ensure CST document retains formatting structure. |
| E020 | `<program>` was not found; callisto requires it to be available | Ensure program is installed and available on system PATH. |
| E021 | `<program>` reports version `<found>`, but callisto requires <required> | Upgrade program to meet version requirement. |
| E022 | Executing `<program>` is not supported on this surface: <reason> | — |
| E023 | `<program>` failed with exit code <exit_code>: <stderr> | — |
| E024 | Failed to run `<program>`: <message> | — |
| E025 | `<program>` timed out after <seconds>s | The process did not exit within the allowed time. This usually indicates a network stall or a registry that is unreachable. |
| E026 | Commit walk failed: <message> | — |
| E027 | Internal invariant violated while operating on a manifest path | — |
| E028 | Manifest dependency has a TOML value that is neither a string nor a table; refusing to silently no-op the rewrite | Fix the dependency's TOML shape to a plain string or a table before running callisto again. |
| E029 | `<raw>` is not a valid <grammar> version: <message> | Ensure the version string strictly adheres to the <grammar> specification. |
| E034 | Cannot compare a <left> version with a <right> version | All version comparisons in a cascade step must share the same version grammar. |
| E054 | Reference `<ref_name>` was not found | Check if the reference or tag exists in local or remote Git refs. |
| E141 | Release PR repository `<repository>` is invalid | Use the exact GitHub owner/repository identity, for example `orin-dx/callisto`. |
| E142 | Release PR <kind> `<branch>` is not a safe Git branch name | Use a non-empty Git ref name without whitespace, control characters, `..`, `@{`, or shell-sensitive prefixes. |
| E143 | Release PR snapshot contains the same pull request number more than once | Refresh the forge snapshot and pass each open pull request exactly once. |
| E144 | Release PR snapshot does not match the configured repository or base branch | Collect the snapshot for the configured repository and base branch immediately before calling Callisto. |
| E145 | Managed release PR #<number> comes from foreign repository `<repository>` | Do not treat a fork branch as Callisto-managed; close or rename the lookalike before retrying. |
| E146 | Found <count> open managed release PRs | Resolve the ambiguous managed PRs manually before running the release action again. |
| E147 | Managed release branch `<branch>` has an invalid SHA suffix | Use the canonical branch or the canonical branch followed by `--` and one lowercase forty-hex commit SHA. |
| E148 | Release PR snapshot changed after Callisto made its decision | Re-run `callisto release-pr decide` with a fresh snapshot; no forge mutation was made. |
| E149 | Commit plan for base commit `<base_commit>` has no file changes | An empty commit plan means nothing was staged; check the version command actually ran. |
| E150 | Path `<path>` has unsupported Git mode `<mode>` | The forge commit API can only create plain, non-executable regular files; commit an executable bit, symlink, or submodule change through a privileged token instead. |
| E151 | Path `<path>` has unsupported change kind `<kind>` | Renames, copies, type changes, and unmerged paths cannot be expressed as forge commit additions/deletions; stage a plain add/modify/delete instead. |
| E152 | Commit plan is <bytes> bytes, exceeding the <limit> byte limit | Split the release into smaller changesets, or reduce large generated files (for example a monolithic CHANGELOG) before retrying. |
| E153 | Path `<path>` may not appear in a forge commit plan | The executor never writes `.github/workflows/*`, `.git/*`, or unsafe paths through the forge commit API; those are inherited unchanged from the base commit. |
| E154 | Pull request #<number> targets the internal staging branch | The `<release-branch>--staging` branch is reserved for the executor's own commit staging; close or rename a pull request opened against it before retrying. |

## callisto-format

| Code | Meaning | Fix |
| --- | --- | --- |
| E035 | Bump_version requires a SemVer version; `<raw>` was parsed as <grammar> | — |
| E036 | No versioning implementation exists for <grammar> | — |
| E037 | Bump requires a PEP 440 version; `<raw>` was parsed as <grammar> | — |
| E038 | Internal error computing bumped version `<raw>`: <message> | — |
| E039 | Version component overflow while bumping `<raw>` | — |
| E040 | Changeset does not start with a `---` frontmatter delimiter on line 1 | Add a `---` frontmatter delimiter on line 1 of the changeset file. |
| E041 | Frontmatter opened with `---` on line 1 but was never closed with a matching `---` | Ensure frontmatter block closes with `---`. |
| E042 | Line <line>: quoted name is never closed with a matching `"` | — |
| E043 | Line <line>: quoted name `<raw>` is followed by unexpected content before the `:` separator | — |
| E044 | Line <line>: no `:` separator found in <raw> | — |
| E045 | Line <line>: package name is empty | — |
| E046 | Line <line>: invalid severity for package <name>: <source> | — |
| E047 | Line <line>: package <name> is named more than once in this changeset's frontmatter (first on line <first_line>) | — |
| E055 | Changeset has entries but an empty or whitespace-only summary | Add a non-empty summary after the closing `---` delimiter. |
| E056 | Cannot write changeset: entries present but summary is empty or whitespace-only | Provide a non-empty summary describing the change. |
| E057 | Entry <index> has an empty package name | — |

## callisto-vcs

| Code | Meaning | Fix |
| --- | --- | --- |
| E050 | Failed to discover Git repository at `<path>`: <message> | Ensure target directory is inside a valid Git repository. |
| E051 | Git error: <0> | — |
| E052 | Reference `<ref_name>` was not found | Check if reference or tag exists in local or remote Git refs. |
| E053 | Tag glob pattern `<pattern>` is not a valid glob: <message> | Fix the glob syntax (e.g. balance `{`/`}` and `[`/`]`) or use a literal tag name. |
| E059 | Staged content for `<path>` no longer matches the index (worktree bytes hash to a different blob than the staged `<expected_sha>`) | Re-stage the file with `git add` so the worktree matches the index, or read it again after staging. |

## callisto-changelog

| Code | Meaning | Fix |
| --- | --- | --- |
| E060 | Changelog rendering requires at least one entry, but received empty input | — |
| E061 | Cannot render a changelog entry with `Severity::None` | — |
| E062 | Failed to read the changelog file | — |
| E063 | Failed to write the changelog file | — |

## callisto-graph — config (`callisto.toml`)

| Code | Meaning | Fix |
| --- | --- | --- |
| E110 | Failed to read `callisto.toml` | — |
| E111 | `callisto.toml` is not valid TOML | Verify callisto.toml TOML syntax formatting. |
| E113 | A package's `changelog` path is absolute or contains `..`, and would escape the workspace root | Use a forward-slash-separated path relative to the package root that does not contain '..' components. |
| E116 | `changesets.dir` is absolute or contains `..`, and would escape the workspace root | Use a forward-slash-separated path relative to the workspace root that is not absolute and does not contain '..' components. |
| E197 | Invalid `[release]` product configuration | Configure one supported product package and its artifact targets. |

## callisto-graph — workspace graph

| Code | Meaning | Fix |
| --- | --- | --- |
| E030 | Workspace root not found between `<path>` and the Git repository root `<path>` | Callisto never searches above the Git repository root. Add a workspace manifest (Cargo.toml with [workspace], package.json with a workspaces field, pnpm-workspace.yaml, or a .moon directory) or a package manifest (Cargo.toml with [package], package.json, or pyproject.toml) inside the repository. |
| E031 | Failed to walk filesystem under `<path>`: <message> | — |
| E032 | Project path `<path>` is outside the workspace root `<root>` | — |
| E033 | VCS error during workspace location: <0> | — |
| E058 | `<path>` is not inside a Git repository | Callisto needs a Git repository: run `git init` in the workspace root. |
| E209 | Failed to parse manifest `<path>`: <message> | Fix the manifest's syntax; a workspace member's manifest is never skipped. |
| E210 | A `.changeset/*.md` file failed to parse: `<path>`: <source> | — |
| E100 | Package ID is defined at more than one manifest path | Ensure package IDs are unique across workspace manifest paths. |
| E102 | Named package was not found in the workspace | Check the package's name and ecosystem, or that its directory has a manifest callisto discovers. |
| E103 | Bare package name is ambiguous; more than one candidate matches | Qualify the name with its ecosystem, e.g. `cargo/pkg` or `cargo:pkg`. |
| E104 | Dependency cycle detected among workspace packages | Refactor workspace dependencies to break the cyclic dependency chain. |
| E105 | Version cascade failed to converge after its iteration limit | Check for oscillating peer or linked group dependencies. |
| E106 | Fixed group's members have divergent on-disk versions | Align on-disk versions for all members of the fixed group. |
| E107 | Group's members use incompatible version grammars | — |
| E108 | Group lists a member not found in the workspace | — |
| E109 | Package is listed in more than one conflicting group | — |
| E114 | Failed to parse `.changeset/pre.json` | Check that .changeset/pre.json is valid JSON and was not partially written. Delete the file and re-run `callisto pre enter` to recover. |
| E115 | Failed to read `.changeset/pre.json` | Check that .changeset/pre.json is readable. Delete the file and re-run `callisto pre enter` to recover. |
| E117 | Manifest's on-disk version doesn't match the version plan's expected from/to version | The manifest version does not match the plan's from or to version. This may indicate the manifest was modified outside of callisto after the plan was generated. |
| E118 | Package declares platform targets through both `napi.targets` and `[tool.maturin].targets` | Remove one of the two target declarations -- either napi.targets in package.json or [tool.maturin].targets in pyproject.toml -- from the package's manifest. |
| E119 | Package's `publish-to` target ecosystem doesn't match its detected ecosystem | Remove the mismatched target from publish-to, or fix the [[package]]/[[package-set]] rule so it only matches packages in that ecosystem. |
| E120 | Package's `publishConfig.registry` (npm) is not an operator-approved registry | `publishConfig.registry` in package.json is manifest-controlled data (a PR author can set it in their own package.json), not operator config, so it is never trusted verbatim as a publish destination. The URL must use the `https` scheme and must exactly match a `url` configured on an `npm`-kind entry in `[registries]` in callisto.toml. Add the registry there if it is a legitimate private registry, or remove the override from package.json. |
| E121 | A canonical manifest's on-disk version differs from its version at `HEAD` while a changeset is still pending there -- a prior `version` run wrote and staged its changes but never committed | Finish the interrupted run (review the staged changes and `git commit` them), or reset it (`git reset --hard HEAD`, discarding the staged work) before running `version` again. |
| E122 | A filesystem operation failed while applying the version plan | Check file permissions and that the path still exists, then re-run `callisto version`. |
| E123 | Release intent references a package that is not an exact selected workspace package | Rebuild the release intent from the current workspace instead of reusing a selection from another workspace. |
| E124 | Release intent no longer matches the current workspace snapshot | Regenerate and reapprove the release intent; no release operation was authorized. |
| E125 | Cannot read a release input file | Ensure the release manifest and configuration files remain readable until validation completes. |
| E126 | Registry binding is not a credential-free canonical URL | Use a URL without userinfo, query parameters, or fragments in callisto.toml. |
| E127 | Release execution state is invalid | Regenerate the release intent; no release operation was authorized. |
| E132 | Git push remote is unsafe | Configure origin with a credential-free HTTPS or SSH URL. |
| E136 | Artifact manifest is not authorized by this release intent | Regenerate the artifact manifest for this exact release intent; no artifact was uploaded. |
| E137 | Artifact path is unsafe (e.g. escapes the artifact directory) | Place the built asset directly beneath the explicit artifact directory without symbolic links. |
| E138 | Cannot read a build artifact | Ensure the build artifact is readable and has not changed since it was attested. |
| E139 | Artifact doesn't match its manifest digest or byte length | Rebuild and re-attest the artifact; it was not uploaded. |
| E140 | GitHub could not verify the artifact's attestation | Verify that the artifact was built by the exact trusted workflow and source commit declared in the release intent. |
| E157 | Registry reported a package version published, but does not yet show it | The publish command already ran and the registry client reported success; this is registry propagation lag, not an unauthorized or stale operation. Re-run reconciliation once the registry catches up -- do not regenerate the release intent. |
| E158 | Release intent could not be constructed | Fix the reported release-intent construction problem (for example, an unsupported execution trust profile) and rebuild the release intent from a valid decision. |
| E159 | Release operation could not be constructed | Fix the reported release-operation construction problem in the source release decision or package graph. |
| E160 | Release decision could not be constructed | Fix the reported release-decision problem (for example an empty or duplicate roster) and re-derive it. |
| E161 | Release input snapshot could not be constructed | Fix the reported release-input-snapshot problem (for example a duplicated package) and re-derive it. |
| E162 | Release package identifier is invalid | Correct the malformed release package identifier reported here; it must be an exact ecosystem/name pair. |
| E163 | Registry operation for a package failed | The registry itself rejected or could not complete the operation (authentication, rate limiting, or a network failure). Resolve the underlying registry condition and retry; this is not a stale or unauthorized release intent. |
| E164 | A release subprocess command failed | Inspect the reported program, arguments, and stderr to diagnose why the release subprocess failed or produced output that could not be parsed. |
| E165 | Release commit verification failed | The committed release decision, changelog, or manifest diff does not match what this commit claims to release. Reconcile the commit's contents with its release decision instead of regenerating the intent. |
| E166 | Cannot decode the committed release decision file | Restore the committed release-decision file from a known-good commit; it was not treated as authorizing a release. |
| E167 | Remote release state (tag, forge release, registry, or artifact) conflicts with this release intent | A tag or forge release already exists remotely with content that differs from this release intent. Reconcile the remote state by hand -- this intent's authorization is not in question. |
| E168 | Release feature/combination is not implemented | This combination is not implemented for release; adjust the release configuration to use a supported combination. |
| E169 | Release `--package` selection is invalid | Select each package at most once, only packages with a pending release and a publish target, and every unreleased platform package a selected npm package depends on. |
| E170 | Release precondition is unmet (e.g. detached HEAD, missing GitHub remote) | Satisfy the reported precondition (for example a detached HEAD or a configured GitHub remote) before retrying. |
| E171 | Internal release invariant violated | This is an internal callisto defect, not an operator action; report it along with the full error detail. |
| E172 | Release execution is incomplete: one or more operations lack verified terminal success | Rerun the release: it adopts every operation that already landed and retries the rest. Do not treat this release as successful. |
| E175 | Configured artifact repository doesn't match the prepared GitHub push remote | Use the repository derived from the trusted Git remote; Callisto will not upload product assets to a caller-selected repository. |
| E176 | Cannot dispatch a release operation because its provider observation is indeterminate | Restore provider credentials or connectivity, then retry. Callisto will not dispatch an effect while it cannot determine the remote identity. |
| E177 | Provider observation is not usable as release evidence | This is an internal callisto defect: a provider adapter produced evidence that does not belong to the operation's role. Report it with the full error detail. |
| E178 | Release run envelope is not valid for this intent | Re-plan the release intent, or run execute with the orchestration revision and artifact manifest the intent was planned against. |
| E179 | An asset's owning package is not part of this release | Release the owning package in the same run: add a changeset for it, or put it in the product's [[fixed-group]]. |
| E180 | Remote refused a tag push: a GitHub App token cannot push a commit whose `.github/workflows/` differs from every branch tip | This happens when releasing a commit that is not a branch tip (for example, recovering an older release) with GITHUB_TOKEN, which cannot be granted the `workflows` scope. Push the tag, and every other tag of this release, as an annotated tag at the target with a non-App credential (a PAT or deploy key), then re-run the release. |
| E187 | Configured forge repository doesn't match the origin remote | The release plan requires [release].forge-repository to be origin's GitHub repository. Use that `owner/repo`, or point origin at the repository binaries release to. |
| E188 | No package produces a binary artifact; nothing to ship | Remove --artifact-target, or add a binary target ([[bin]], an npm `bin`, or [project.scripts]). |
| E189 | `--product-package` value is invalid | Name one of the binary-producing packages (candidates are listed in the error). |
| E190 | `callisto init` target already exists; workspace is already initialized | Edit callisto.toml directly; `callisto init` only scaffolds a workspace without one. |
| E191 | Workspace root is not a Git repository | Run `git init` in the workspace root, then re-run `callisto init`. |
| E192 | No `origin` remote is configured | Add the repository's remote as `origin`: `git remote add origin <url>`. |
| E193 | Package matches more than one tag naming convention | Write a [[package]] entry for it with `tag-template` set to the current convention and `previous-tag-templates` listing the older ones. |
| E194 | More than one package shares a `v{version}` tag | Give each package its own [[package]] `tag-template` (and `previous-tag-templates` for the shared `v{version}` tags). |
| E195 | `--forge-repository` value is invalid | Use the GitHub `owner/repo` the product releases to. |
| E196 | `--artifact-target` triple is invalid | Use distinct, non-empty Rust target triples. |
| E198 | `[release]` declares no `forge-repository` | Add forge-repository = "owner/repo" under [release] in callisto.toml. |
| E199 | Package configures a `publish-to` target that release cannot dispatch yet | Remove the target from `publish-to` for this package, or publish it outside `callisto release`. |
| E200 | Generated workflow file already exists; `callisto init` refuses to overwrite it | Remove or edit the existing file directly; `callisto init` never merges into it. |
| E201 | Couldn't resolve `callisto@{version}` to a commit on `orin-dx/callisto` | check network access to github.com, or run `callisto init --no-workflow` to skip workflow generation |
| E202 | `callisto init` cannot generate a release workflow for this workspace | omit --workflow and write .github/workflows/callisto-release.yml by hand |
| E203 | Package declares platform targets and also ships `[[release.artifact]]` binaries; only one is allowed | Build the native addon and the release binaries from separate packages, or drop one of the two declarations. |
| E204 | napi package needs exactly one napi `cdylib` crate matching its binary name, and none or more than one candidate exists | Set `napi.binaryName` in package.json to the `[lib] name` (or package name with `-` as `_`) of the one crate that builds the addon. |
| E205 | Fixed group aligns on a tagged member that has no base version in the workspace | Ensure the tagged group member is still a live workspace package, or re-tag against a current member. |
| E206 | Fixed group has no live members with a base version to align on | Ensure at least one member of the fixed group resolves to a workspace package. |
| E207 | Computed bump for a package would move its version backwards | This indicates a corrupted alignment base (bad tag, pre.json, or group config); verify release state before retrying. |
| E208 | A lockfile refresh command exited non-zero while applying the version plan | Run the named command to see the full failure, fix it, then re-run `callisto version`. |

## callisto-cli (`callisto::*` namespace)

| Code | Meaning | Fix |
| --- | --- | --- |
| `callisto::registry_error` | Wraps a registry client error (message is the underlying error's own text) | verify registry credentials/authentication and network connectivity, then retry |
| `callisto::pre_json_error` | Wraps a `.changeset/pre.json` parse error (message is the underlying error's own text) | — |
| `callisto::io_error` | An I/O error, optionally naming the path being accessed | check that the path exists and that you have permission to access it |
| `callisto::not_a_tty` | Refusing to prompt interactively: stdin is not a terminal and no non-interactive flags were given | specify package names explicitly via `callisto add --package <name>:<severity>` in CI environments |
| `callisto::release_intent_schema_unsupported` | Release intent's schema version doesn't match what this build reads | re-run `callisto release plan` to derive a fresh intent: an intent is bound to one build's operation graph and is never reused across versions |
| `callisto::release_artifact_manifest_dry_run` | `release artifact-manifest` was run with `--dry-run`, but it needs an output file | re-run `callisto release artifact-manifest` without --dry-run |
| `callisto::release_plan_dry_run` | `release plan` was run with `--dry-run`, but planning is already read-only and needs an output file | re-run `callisto release plan --out <file>` without --dry-run |
| `callisto::release_execute_dry_run` | `release execute` was run with `--dry-run`, which it doesn't support | remove --dry-run; release execute has no read-only mode |
| `callisto::release_no_artifact_slots` | Release intent declares no binary artifact slots; no artifact manifest can be created | plan the release with --orchestration-revision and --artifact-repository so the intent declares artifact slots |
| `callisto::release_manifest_source_not_git` | Artifact manifests require a Git commit release source | plan the release from a Git commit source |
| `callisto::release_artifact_not_regular_file` | A declared artifact isn't a regular file directly in the artifact directory | place the built asset as a plain file (not a symlink or directory) directly in the artifact directory |
| `callisto::release_artifact_escapes_directory` | A declared artifact resolves outside the artifact directory | remove symlinks so every asset resolves inside the artifact directory |
| `callisto::release_manifest_creation_failed` | Cannot create the artifact manifest | check that the artifact directory holds exactly the assets the intent declares |
| `callisto::release_commit_invalid` | `--from-release-commit` value is not a valid merged release commit | pass the full commit SHA of the merged release commit |
| `callisto::release_decision_outside_source` | `--decision` file is outside the selected `--source-root` | point --decision at a file inside the source root selected with --source-root |
| `callisto::release_decision_path_invalid` | `--decision` path is invalid | pass a repository-relative decision path without `..` components |
| `callisto::release_package_invalid` | `--package` value is not a valid ecosystem-qualified package identity | name each package as <ecosystem>/<name>, for example cargo/callisto-cli |
| `callisto::release_orchestration_revision_invalid` | `--orchestration-revision` value is invalid | pass the full commit SHA of the orchestration workflow revision |
| `callisto::release_artifact_repository_invalid` | `--artifact-repository` value is invalid | pass the forge repository as <owner>/<repo> |
| `callisto::release_forge_repository_mismatch` | `--artifact-repository` doesn't match `[release].forge-repository` in callisto.toml | plan with an --artifact-repository matching [release].forge-repository in callisto.toml |
| `callisto::release_orchestration_flags_required` | Product release planning requires both `--orchestration-revision` and `--artifact-repository` | pass both --orchestration-revision and --artifact-repository |
| `callisto::release_unexpected_artifact_inputs` | `--artifact-manifest`/`--artifact-dir` given, but the intent declares no artifact slots | drop --artifact-manifest and --artifact-dir for an intent without artifact slots |
| `callisto::release_missing_artifact_inputs` | Intent declares artifact slots, but `--artifact-manifest`/`--artifact-dir` are missing | pass both --artifact-manifest and --artifact-dir |
| `callisto::release_envelope_invalid` | Release run envelope is not valid for this intent | re-check the orchestration revision and artifact manifest against the intent |
| `callisto::release_receipt_issue_failed` | Cannot issue the terminal release receipt | re-run `callisto release execute`; it adopts effects that already landed |
| `callisto::release_intent_invalid` | Release intent file is invalid | re-run `callisto release plan` to derive a fresh intent |
| `callisto::release_artifact_manifest_invalid` | Artifact manifest file is invalid | re-run `callisto release artifact-manifest` to regenerate it |
| `callisto::release_json_invalid` | A release JSON file (intent, decision, manifest) is not well-formed | check the file is complete, well-formed JSON |
| `callisto::release_requires_ci_route` | This workspace cannot release locally | release it from CI with `callisto release plan`, `callisto release artifact-manifest`, then `callisto release execute`; `callisto release --dry-run` still previews it locally |
| `callisto::init_requires_yes` | stdin is not a terminal, so `init` needs flags instead of prompts, and required flags are missing | re-run `callisto init --yes` with the listed flags, or run it in a terminal to be asked |
| `callisto::init_missing_flags` | `init --yes` is missing required flag(s) | supply the listed flags; see `callisto init --help` |
| `callisto::init_workflow_flags_conflict` | `--workflow` and `--no-workflow` are mutually exclusive | pass only one of --workflow or --no-workflow |
| `callisto::error` | Fallback/untyped error; message is whatever string was wrapped | — |

186 codes total: 153 numeric (45 callisto-model, 18 callisto-format, 4 callisto-vcs, 4 callisto-changelog, 82 callisto-graph: 5 config + 77 graph) + 33 `callisto::*` in callisto-cli.
