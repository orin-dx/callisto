# callisto-cli

## 0.16.0

- Pinning a version over `version.workspace = true` keeps the comment above that line and the one after it.
- `version` now changes only the updated values in `package.json`, keeping inline arrays and objects in place for smaller release PR diffs.
- New `callisto release pending` lists merged release commits whose release never finished (a missing tag or an unpublished GitHub release), oldest first, so a release that stopped partway can be found and recovered. It reads the committed decision of each release commit and publishes nothing.
- Callisto now blocks a first release when the registry links the package name to another repository.

  - The error names that repository.
  - If the package moved and is yours, tag its version commit before retrying.
- Publishing with Yarn now honors `publishConfig.registry` without failing on an unsupported `--registry` flag.
- Release planning is about twice as fast in large workspaces, including with `--dry-run`.
- Empty snapshot tags, or tags containing spaces or `/`, now fail with a clear fix instead of producing a malformed version or an internal error.
- Poetry dependency updates now preserve nearby comments.
- `init` now explains that defaults apply when there is no configuration to write.
- npm ranges with spaced comparators are now recognized. If a new dependency version falls outside the range, `version` stops and asks you to update it instead of silently leaving the dependent unchanged.
- A crate's first release now checks for a description and a license or license file before publishing, avoiding a partial release.
- Generated changelogs and release PR details now keep blank lines clean around nested bullets.
- `release` now warns when a package's latest tag exists locally but not on `origin`, including in `--dry-run`. This exposes an unfinished tag push instead of reporting `Nothing to release.`
- Cargo dependency updates now keep comments immediately above a version, including shared entries in `[workspace.dependencies]`.
- `release --dry-run` now returns its plan in a few seconds when an optional registry name check cannot respond.
- `init` now supports repositories with all packages in subdirectories and no top-level manifest.
- A release note that contains a heading line, or a code block opened on a bullet, is no longer cut short or run on into the older versions when it is shown or published.
- Release notes no longer stop at headings inside code blocks or indented list items. Changelogs with a byte order mark no longer gain a duplicate title.
- Callisto's GitHub release workflow can finish an interrupted release after `main` advances.

  - It resumes from the merged release PR's commit.
  - It finishes older pending releases before preparing the next release PR.
- Python `~=` dependency updates keep the original precision: `~=1.2` becomes `~=2.0`, leaving later 2.x releases eligible.
- Named Cargo registry tokens now reach Cargo only. Git, the GitHub CLI and other publishing tools no longer inherit them.

## 0.15.0

- `callisto init --yes` now works with no other flags: packages version independently unless `--versioning fixed` is given, and a GitHub repository gets the release workflow unless `--no-workflow` is given. A single package is no longer asked how to version. If the unrequested workflow cannot be built, such as offline, `init` skips it with a warning instead of failing. After writing the workflow, `init` lists the repository secrets to add and the first commands to run, and the publishing guide has new sections on first publishes and on pulling a release.
- The GitHub release of a package in a fixed group now lists the changes recorded against the group's other members, each marked with its package, instead of only its own changelog section.
- A generated build matrix builds `x86_64-apple-darwin` on `macos-latest`, since GitHub has retired the `macos-13` runner it used. The proto plugin now installs the Intel macOS and arm64 Linux release binaries.
- A changeset with an entry that names no package in the workspace, such as a typo or a removed package, now applies none of its entries and says so. Before, its other entries were applied and the file stayed on disk, so every `version` run bumped those packages again. A Cargo crate with no version, such as an xtask, no longer stops every run with E013: it is skipped with a note.
- A release commit is now checked for what else it changes. Verification (E165) refuses a version commit that touches any path other than manifests, lockfiles, changelogs, changesets, `pre.json` and the decision file, so a release pull request can no longer carry a change to source, a build script, `.npmrc` or `.cargo/config.toml` under the cover of a bot-authored diff. Move such edits to a separate commit.
- The leftover `[init]` and `[release.profiles]` tables in `callisto.toml` are removed, and a config that still has one now fails to load, naming the key; move a profile's `forge-repository` to `[release]`. The no-op `setup_git_user` action input is removed, and the setup action no longer falls back to building unreleased `main` when a release cannot be installed. The job's OIDC request variables now reach only the package-manager tools, not cargo, git or gh.

## 0.14.2

- A release no longer waits past its confirmation window when a registry keeps asking to retry later. The run now gives up when the window ends, as the error already said.
- The help for `add --package` and `matrix --package` now shows the `ecosystem/name` form they require, and the contributor and publishing guides match.

## 0.14.1

- Release tags now record the package that made them and the git tree of each of its directories in the tag message, as Callisto-Package and Callisto-Tree lines. Nothing reads them yet, and tags without them keep working.
- Waiting for npm packages to appear is now one window of confirm-timeout for the whole release: a package that shows up along the way no longer restarts it, and the time spent checking the registry counts, so a slow registry cannot stretch the wait to several windows.
- `status`, `version` and `release` skip two history reads for tags made by a recent release. They still read them for every other tag, so older and hand-made tags behave as before.

## 0.14.0

- After publishing, Callisto looks for a new npm version right away and then at 15 seconds, 45 seconds, 2 minutes and 3 minutes, instead of every 30 seconds, so a release makes about five looks per window rather than seventeen. The window and `confirm-timeout` are unchanged.
- A release no longer stops with E319 when a dependent's range already covers the new version but cannot be raised, such as `*` or a bare Python requirement. A range that a rewrite would leave excluding the new version, such as `<2.0.0` for 2.0.0, now fails with E319 instead of being published.
- A `[[package]]` or `[[package-set]]` rule is now matched against a package's name in the rule's own ecosystem. For a dual Cargo and npm package whose npm name differs, such as the napi package `@orin-dx/michi` in a directory called `michi-node`, `npm/michi-node` used to apply to it silently while a `[[fixed-group]]` rejected the same spelling. It now matches nothing, the matched-no-packages warning suggests `npm/@orin-dx/michi`, and a group member's E108 does too.
- A `[registries.<name>]` entry for a custom registry must now set `kind` to `cargo`, `npm` or `pypi`; without it Callisto fails with E321 instead of silently reading the registry as npm. The built-in `cratesIo`, `npm` and `pypi` entries may leave it out and keep their own kind, so `[registries.cratesIo]` with only a `confirm-timeout` no longer turns the crates.io registry into an npm one.
- A release of several npm packages now waits for the registry once instead of once per package. Callisto publishes the npm packages back to back, a package after the dependency it follows without waiting for the registry to show that dependency, then looks at all of them together until each version is visible. Tags, GitHub releases and uploads still wait until every npm version they follow is shown, so nothing is announced early. A release of eight npm packages that took eight waits of up to six minutes now takes one. Cargo and PyPI publishes are still confirmed one at a time.
- status, version and release --dry-run read tags and manifests in a handful of git calls instead of several per package, so they take well under a second on workspaces with hundreds of packages.
- Workspaces with thousands of packages load and plan about a quarter faster: manifests are read and parsed on several threads, and package name lookups no longer slow down as the package count grows.
- A local release no longer tags a published version onto code that was never published when the package inherits its version from the workspace, has more than 100 edits to its manifest, or was merged after unrelated commits; a registry that cannot be read now stops the release. Signed commits no longer break change detection when log.showSignature is set.
- status decides whether a package changed since its last tag by comparing its directories with the tagged ones, so it takes a fraction of a second however many commits the repository has, and a change that was later reverted no longer counts as a change.

## 0.13.0

- The release pull request no longer says "All changes are backward compatible" when a package is below 1.0.0 and gets a minor bump. It says that minor bumps below 1.0.0 can include breaking changes and to read the changes first.
- A registry in callisto.toml can set `publish-timeout` (for example `publish-timeout = "5m"`) to change how long one publish command may run before Callisto stops it with E025. The default stays 15 minutes.
- When a run adopts a Cargo or npm version that is already published, Callisto now warns if the registry records a different repository for it than this repository's origin, since the name may belong to someone else's package. It still adopts the version. A registry that records no repository, and PyPI, are not checked.
- Release binaries are now published for Intel macOS (`x86_64-apple-darwin`) and arm64 Linux (`aarch64-unknown-linux-gnu`) as well as the existing three, and the `setup-callisto` action downloads them on those machines instead of building from crates.io. An older release without the new asset still installs from crates.io.

## 0.12.0

- A `pypi` registry in `callisto.toml` can set `publish-url` when its upload endpoint differs from its simple index: Callisto reads releases from `url` and uploads to `publish-url`. Without it, `url` serves both, as before.
- A failed npm, Cargo or PyPI publish now shows what the publish tool printed to standard output, not only standard error. pnpm prints its errors there, so a failed pnpm publish used to end with `npm publish failed (exit 1):` and nothing after it. The output also appears live in the release log while the publish runs, instead of only after it ends.
- A release whose changelog exists but cannot be read now fails planning with E125 naming the file, instead of publishing generated notes in place of the changelog the release PR reviewed. A missing changelog, a missing version section and an empty section still fall back to generated notes, with the notice.
- A package that takes a name from a renamed or removed package, including one in another ecosystem, no longer takes over that package's pre-release baseline or release tags, so it is no longer skipped as already released. `status` says which tag was rejected and points to `previous-tag-templates`.
- A `[[fixed-group]]` or `[[linked-group]]` that lists one package under two of its names, such as a Cargo crate and its npm name in a dual package, is now rejected with E250 instead of being accepted silently.
- When a release stops because a registry or forge could not be observed (E176) or a publish is not yet visible (E157), the error names the operation in words, such as "tag of cargo/demo 0.10.0" or "registry publish to internal of npm/demo 1.0.0", and ends with the most recent failed command and the last 20 lines or 2 KiB of its stderr, with credentials redacted.
- A release no longer stops with E157 when npm shows a published version late. After each publish Callisto now waits for the version to appear, up to 6 minutes for npm and PyPI and 2 for Cargo, instead of about 30 seconds, and the wait applies to a rerun that finds the version already published. A registry that is slower can set `confirm-timeout` on its `[registries]` entry in callisto.toml, for example `confirm-timeout = "10m"`.
- `callisto version` now warns about a changeset entry that names a package that is not in the workspace, as `status` already did. The changeset stays on disk, and its other entries are applied again by the next `version` run, so fix or remove the entry.
- Inference from commits now stops with E315 in a shallow clone instead of planning a smaller bump from the history it can see. A workspace table written inline or with dotted keys is recognised as a Cargo workspace root, and a pnpm workspace is published with pnpm even where its lockfile is not committed. An unreadable `src/bin` no longer hides a binary from `callisto init`, and the napi target check reads the main package's own `package.json` wherever it is.
- `callisto --help` describes Callisto as a release manager for Cargo, npm and Python workspaces instead of Rust workspaces only.
- In pre-release mode, `version` no longer bumps a package again when its only changesets are already recorded in `pre.json`: a rerun with nothing new changes nothing, and `status` stops reporting it as pending. A new changeset produces the next prerelease. `release-pr decide` now refuses pre-release mode with E318 and a local-release fix, instead of opening a pull request that cannot release.
- npm prereleases now publish under the tag their version names: `1.0.0-beta.2` goes to `beta`, `2.0.0-rc.1` to `rc`, and `next` remains the fallback, instead of every prerelease landing on `next`. `release` now refuses a version that `callisto snapshot` wrote (E169), so a snapshot is never tagged or published.
- Workspaces below the repository root, such as `native/` or `rust/`, are now supported for Cargo, npm, pnpm and uv. A nested workspace decides which packages beneath it are packages by its own `members` and `exclude`, `workspaces`, `pnpm-workspace.yaml` or `[tool.uv.workspace]`, and a private versionless `package.json` that holds one is not a package. Cargo members inherit `version.workspace = true` and `{ workspace = true }` dependencies from it instead of failing with E313. `callisto version` refreshes and stages the lockfile nearest to each package, and a release commit is verified against it. A nested npm workspace without a lockfile publishes from its own directory, a lockfile beside a package no longer takes it out of its workspace, and `npm view` and a publish by directory run in that directory, so the version looked for and the version published come from the same `.npmrc`. A package that moves to a different `package.workspace` between two commits is verified against each commit's own workspace, an interrupted-run check finds a staged bump in the workspace the package named at `HEAD`, and a directory that cannot be opened for syncing after a write is reported instead of skipped.
- An error from a failing `git` or other helper command run while versioning no longer prints a credential that the command echoed, such as a token in an authenticated remote URL. Registry tool output shown in errors and the CI log also hides more credential shapes: `npm_`, `ghp_` and `github_pat_` tokens, PyPI API tokens, `Bearer` values and `.npmrc` auth lines, and the value of any environment variable named like `*_TOKEN`, `*_SECRET` or `*_PASSWORD`, even when it is not one of the variables Callisto reads itself.
- A directory that holds both a Cargo or npm manifest and a Python manifest now fails with a message that names the grammars in a fixed order and says to put the Python package in its own directory.
- A library that ignores its `Cargo.lock` can release again: `callisto version` refreshes the lockfile but no longer fails trying to stage it. A changelog or manifest that a `.gitignore` ignores now stops `version` before anything is written, naming the file, instead of failing after the edits. Two `callisto add` runs at the same moment can no longer replace each other's changeset.
- Package references now take the form `ecosystem/name`, such as `cargo/foo`, `npm/foo` or `pypi/foo`. The colon form, such as `cargo:foo`, is rejected with E316 naming the slash form, in `--package`, changeset entries, `pre.json` keys, group members, package rules, and the product and artifact packages. A bare name is rejected with E317 showing the form to write, in `callisto.toml` (a group, a `[[package]]` or `[[package-set]]` `match`, a product or artifact package) and in `callisto add --package` and `callisto matrix --package`, so a name can no longer become ambiguous when a package in another ecosystem later takes it; `release --package` already required it. A package-set glob names one ecosystem, so a rule for several ecosystems is one rule each. Changeset entries and `pre.json` keys keep native names.
- `callisto release` now stops with E320 instead of tagging a version that is already on the registry onto a later commit. After a run that stopped between publishing and tagging, a rerun on a newer commit that changed the package names the commit that set the version; checking that commit out and running `callisto release` again adopts the published version and tags it there. The check follows the commit that set the package's own literal version, so a version inherited from the workspace is not covered.
- The `status`, `version`, `snapshot` and `matrix` JSON reports now name every package `ecosystem/name`, such as `cargo/foo`, for its primary ecosystem, whether or not another ecosystem shares the name. This covers bumps, cascade reasons, diagnostics and the `matrix` map keys. These four reports now carry `schemaVersion` 2; the other reports still carry 1. Text output follows. Changeset entries, `pre.json` keys and default tags are unchanged.
- `callisto schema` now prints JSON Schema draft 2020-12: shared definitions live under `$defs` instead of `definitions`, and a fixed string variant is written as `const` instead of a one-element `enum`. The shapes the schemas accept are unchanged.
- A Poetry caret or tilde dependency on a Python package released together with it, such as `core-py = "^0.1.0"`, is now rewritten to cover the new version instead of being left behind with a warning. A range that spans a single minor, such as `core-py>=0.1.0,<0.2`, stays one minor wide (`>=0.2.0, <0.3`) instead of widening to the next major.
- `status` now warns when a package has a pending release inferred from commits but no changeset names it. The release pull request reads changesets only, so it would never release that package. `status --check --strict` fails on it.
- The release workflow `callisto init` generates for a simple workspace now runs one release at a time, so two pushes to the default branch in a row no longer race to tag and publish the same versions.
- `version` and `snapshot` now stop with E319 before writing anything when a dependency range that consumers install would keep excluding the new version cannot be rewritten, such as a Cargo `>=1.2.0, <2.0.0` range after a major bump. It used to warn, release the dependent anyway and leave an unresolvable range. An npm range with a prerelease clause, such as `^2.0.0-next.0` left over from a prerelease cycle, is now rewritten like any other (`^3.0.0`). Dev dependencies still only warn.
- A package published to several registries now orders each distribution on its own. A dependent follows only the distribution it depends on: a Cargo dependent follows the crate and an npm dependent follows the npm package, so a failed publish in one ecosystem no longer holds back dependents in the other. A product's GitHub release is no longer made public before the registry publishes of a plugin package whose asset it carries have finished. `callisto release --package` on one distribution, such as `npm/dual`, releases every distribution of that package, so its shared tag and GitHub release no longer follow a single publish. An intent planned by an older version is rejected as stale and needs `release plan` again.
- The published crates now declare their minimum Rust version, keywords and categories, and no longer ship their test files, so crates.io shows the right metadata and installs download less.

## 0.11.0

- A Python release that stopped partway is completed on rerun. Only the files PyPI lacks are uploaded, and the release counts as published only when both the wheel and the sdist it builds are on the index, so no tag is created for a half-published release. Uploads name their files instead of a dist glob, so a stale dist directory is no longer published.
  
  Breaking: `callisto_graph`'s `registry_argv::pypi_publish_argv` is replaced by `pypi_build_argv` and `pypi_upload_argv`.
- A package that sits behind a symbolic link is now rejected with an error naming it, because Git cannot stage a change to a file beneath a link and the version bump could never be committed. A link that holds no package is ignored, and a link can be skipped by listing it in a `.gitignore` under the workspace root.
- A failed Git lookup is now an error instead of "nothing there". A broken or missing repository used to read as no tags, no such ref, no file at that revision or no matches, so `status` and `version` could plan from the wrong history without saying so. Only Git's own "not found" answer counts as absent now, a tag that is listed but does not resolve to a commit stops the run, and `status` warns once about each tag that matches a package's tag template but is not a version.
- Generated release workflows now install the Python build and upload tools they need. A workspace that publishes to PyPI no longer depends on `twine` being preinstalled on the runner.
- A package published to both Cargo and npm now gets its tag and GitHub release only after both publishes succeed, instead of after the first.
- `callisto add` no longer overwrites an existing changeset when it draws a name that is already taken, and a file Callisto rewrites keeps its permissions (it was left owner-only). A command whose output was cut short because a background process kept its pipes open is now an error instead of a success with empty output. Workspace globs written as `./packages/*` or `packages/*/` now match the same directories as `packages/*`.
- A package published to both Cargo and npm now gets one git tag and one GitHub release. A release that also uploads assets, which failed to plan with a repeated artifact slot, now plans and runs.
- A PyPI token or npm config token that a publish tool echoes in an error is now redacted from the output, like the other registry tokens.
- Callisto now stops, naming the file, when a workspace manifest is malformed instead of guessing which packages it contains. An unreadable or unparseable root `Cargo.toml`, `pyproject.toml`, `package.json` or `pnpm-workspace.yaml`, and a `members`, `exclude`, `workspaces` or `packages` entry that is not a valid list of globs, fail the command; before, they silently admitted every directory, so vendored copies and excluded crates became releasable packages. Locating the workspace root also no longer skips a malformed manifest and settles on a different root.
- **A Cargo dependency declared in several tables is followed and rewritten in each**
  
  A dependency declared more than once, for example in `[dependencies]` and in a `[target.<cfg>.dependencies]` table with different constraints, now cascades to its dependent and is rewritten in every table when the package it names is released. Before, only the first declaration was updated and another one kept a stale constraint. A target declaration whose range still covers the new version now has its lower bound raised, as a plain declaration does. A Python requirement declared more than once, in the project dependencies, in several extras, in Poetry's table or under different markers, is followed and rewritten in each place by its own constraint, so a loose line such as `lib-py>=0.1` has its floor raised to `>=0.2.0` rather than being skipped.
  
  Breaking:
  - `callisto_model`: `DependencyEntry` and `DepEdge` gain a `locator: DepLocator` field naming the table a declaration lives in. `DepLocator` is a `#[non_exhaustive]` enum, `Unspecified` (the default, for npm), `Cargo` or `Python`: build it with `DepLocator::cargo` or `DepLocator::python`. The new `DepContainer` names the Python table. A writer given a locator for another ecosystem returns `DependencyNotFound`.
  - `callisto_manifests`: `Manifest::update_dependency_spec` takes the declaration's `&DepLocator` after the dependency kind.
  - `callisto_graph`: `RewriteKey` gains a `locator` field.
- Yarn classic's `"workspaces": { "packages": [...] }` form is now read for workspace membership, and a root Cargo workspace that cannot be loaded for version inheritance is reported instead of silently dropping the packages that inherit from it.
- Callisto no longer plans a release from a workspace it could only partly read. A package whose manifest cannot be opened (for example a member that inherits its version from a Rust workspace Callisto cannot find) used to keep no publish target and no dependency edges, so it never published and its dependents never cascaded, without a message. Every command now stops with a coded error naming each such manifest.
- A release workflow that builds Python distributions in a job without registry credentials now publishes exactly those files. The generated build-matrix workflow gains a `build-python` job, the manifest records each file's name, size and sha256, and a file PyPI already holds under a sealed name with different bytes stops the release before any upload. Python builds also set `SOURCE_DATE_EPOCH` to the release commit time.
- callisto status now reads a changeset's package names the same way callisto version does. A package named by a non-primary manifest's own name resolves, a qualified name for an ecosystem the package lacks is reported as unknown, and a name that two packages claim is ambiguous, so status --check no longer passes a changeset that version will not consume.
- A release now stops with a clear error when git cannot read the local tag state, for example a corrupt or unreadable repository, or when npm cannot be reached or rejects the version lookup, instead of treating the version as unpublished and attempting to publish or tag. npm 8 and later still report a missing version as unpublished.
- A file Callisto cannot read or look up is now reported instead of treated as missing. An unreadable package manifest, `callisto.toml`, `pre.json` or changelog, a changeset it cannot inspect, and a failed directory sync after a write now stop the command with the file named, where before the package, config, changeset or durability guarantee was silently dropped.
- A release that stops because a provider could not be observed now says why: the HTTP status the registry or forge answered, a failed command, an answer it could not understand, or a timeout.
- `version` now checks every file it is about to write before it writes the first one. A manifest that is missing, unreadable, a directory, in an unwritable folder, or reached through a symbolic link Git cannot stage stops the command with every such file named and nothing changed, instead of failing partway through and leaving some manifests already bumped.
- A pre-release state that records two different starting versions for one package, for example an old bare key and its qualified key, now stops with an error naming both keys and versions instead of silently keeping one.
- A package keeps its release history when its directory moves, when it gains a higher-priority manifest beside a same-named package, or when a same-named sibling is removed. Before, the last release was reported as none, or an older release was chosen over a newer one, so already-released work could be released again.
  
  Breaking:
  - `callisto_graph`: `TagIndex::build` takes the workspace's `&IdentityIndex` after the graph.

## 0.10.3

- A release rerun adopts a local tag whose message has several paragraphs or a signature, instead of failing while reading it.
- A rerun after a failed tag push now completes the release. A tag that exists on the release commit no longer hides unfinished work: the push, GitHub release and any other missing steps are performed and the receipt is written.
- The generated build-matrix release workflow builds and places napi addons only for packages in the release. An unrelated or misconfigured napi addon no longer adds builds or fails the plan job of a release that does not include it.

## 0.10.2

- Releases that ship artifacts no longer fail with E140: the GitHub CLI that checks attestations and publishes the release keeps its own token instead of running unauthenticated.

## 0.10.1

- Generated npm and PyPI releases configure native registry authentication for publishing without exposing credentials to Python build backends.
- GitHub Actions honor requested Callisto versions; building checkout source requires source: local.
- Versioning refreshes and stages lockfiles for every package manifest and dependency declaration it updates, including dual Cargo and npm packages.
- Release decisions verify effective package versions from the selected commit, including versions inherited from Cargo workspace metadata, and only treat direct-child changeset Markdown files (excluding README.md) as consumed changesets.
- Interrupted version runs retain release inputs until generated changes and their removal are safely staged, prevent unsafe retries, and explain how to inspect staged work and finish recovery.
- Version and snapshot updates recognize Cargo aliases and equivalent Python distribution names while preserving dependency declaration spelling.
- CLI JSON schemas accurately describe the string values emitted for versions, package identities, and digests.
- Pre-release baselines are validated using each package's native version syntax, including when packages share a name or have been promoted to ecosystem-qualified ids.
- npm version ranges with `||` alternatives are understood for dependency coverage; pnpm workspace aliases, relative references and explicit ranges are resolved and updated without changing their protocol or dependency names.
- Python dependencies pinned in `[project.optional-dependencies]` extras are now followed: bumping a package releases the packages that pin it in an extra and rewrites the pin, instead of leaving stale published metadata.
- Durable release plans read Python package versions using PEP 440 semantics.
- The release PR refreshes reliably when the forge is slow to show the moved branch, instead of failing with E148 after the branch has already been updated.
- Generated artifact workflows include every supported Cargo binary; npm and Python executables remain registry releases unless a custom workflow builds their artifacts. `callisto init` now reports E304 when an npm or Python executable is chosen as the product package, and suggests `--no-workflow` for a registry-only release.
- Cargo dependencies declared under `[target.<cfg>.dependencies]` are now followed: bumping a package releases its target-specific dependents and rewrites their pins, instead of leaving a stale pin that broke the workspace.
- Automated release PRs can refresh using generated decisions without rejecting their own branch updates.

## 0.10.0

- Output that explains why a package is released reports `release-trigger`, fixed groups, linked groups, the tag template and pre-major inference as configured when you set them, instead of "(default)".
- `snapshot` works for PyPI packages: their snapshot version and dependents' specs use PEP 440.
- Git errors that came from parsing git's own output now have specific codes and help text, instead of one generic git error code.
- release, release plan, release execute, release artifact-manifest, and release-pr's JSON output now carry schemaVersion, command, and dryRun fields like every other command's JSON output.
- `callisto release-pr verify` accepts `callisto release-pr decide --format json` output as-is again, so the release action can open and update the release PR. 0.9.0 failed with E281.
- Adding a package with the same name in another ecosystem needs no configuration: the existing package keeps its release history, tags and changelog, and new tags use the qualified name (`cargo/foo@1.1.0`). A pre-release cycle started before the new package keeps working. A changeset that names a now-ambiguous `foo` fails with E103 and shows the qualified form to write.
- init now names the exact file it failed to write when a write fails, instead of a pathless error.
- In pre-release mode, packages with the same name in two ecosystems (`cargo/foo`, `npm/foo`) keep separate starting versions in `pre.json` instead of sharing one.
- `callisto release` no longer fails with E167 when GitHub is slow to show a release or asset it just created, uploaded or published; it waits up to a minute.

## 0.9.0

- callisto release --dry-run and callisto release plan now include a diagnostics array in their JSON output when an artifact owner package is selected without its product package, instead of only printing a stderr warning.
- callisto version now refreshes npm, pnpm, yarn, bun and pdm lockfiles by default (in addition to Cargo, uv and poetry), opt out with --no-refresh-lockfiles; a failed refresh now stops the command with a coded error naming the lockfile and command
- A dependent released in the same run has its spec on a bumped internal dependency raised to the new version, even when the old spec still covers it.
- Generated release workflows pass the detected default branch to the release-PR action, and `init` now resolves it from a live remote query or the checked-out branch when `origin/HEAD` isn't set locally, instead of silently defaulting to `main`.
- **Git runs through the system `git` binary only**
  
  The in-process gitoxide backend is gone; every Git read and write already had to work through the `git` binary the CLI requires. Commit inference keeps a commit that touched a package even when a later merge discarded its change (`git log --full-history`), matching the removed backend. An annotated release tag made with no Git identity configured takes the target commit's committer as its tagger.
  
  Breaking:
  - `callisto_model::vcs` (was `callisto-vcs`): removed `GitRepository`, `ShellGit`, the `GitDataSource` trait and the revwalk visit counter. `GitAccess::discover(root, runner)` is now `GitAccess::new(root, runner)` with inherent methods.
  - `callisto_model::vcs`: `commits_since` with an unresolvable `since` ref returns `VcsError::RefNotFound` (was `VcsError::Git` from the shell path).
- Shell completions no longer offer hidden plumbing subcommands, at any nesting depth
- snapshot rewrites every dependency spec the snapshot version falls outside of, so cargo metadata resolves after a snapshot
- Loading a workspace now fails with a clear error naming both directories when two of them declare the same ecosystem and name, instead of silently routing dependency edges to the wrong package.
- Fixed groups compute their shared version from the highest member version when no member is tagged, support PEP 440 versions, and follow pre-release mode. Before, an untagged group could be bumped down to 0.x.
- `callisto init` can generate `.github/workflows/callisto-release.yml` (the workflow path release artifact attestations are bound to) for a workspace with no napi/maturin platform packages and no shipped release artifacts: interactive question or `--workflow`/`--no-workflow` (mutually exclusive), refuses to overwrite an existing file (checked before the preview, including under `--dry-run`). After writing the file, init prints that a merge to the default branch publishes, so that branch should require pull requests and reviews. JSON output carries the same text as a `workflow-merge-publishes` diagnostic with the new `info` severity. The generated file is two jobs (`version-pr`, `release`) under 40 lines, triggered on push to the detected default branch, with per-job permissions and secrets wired per detected ecosystem (`CARGO_REGISTRY_TOKEN`, `NPM_TOKEN`, `PYPI_TOKEN`; npm workspaces also set `NPM_CONFIG_PROVENANCE` so publishes carry provenance), pinned to `orin-dx/callisto/.github/actions/callisto-action@<commit>` -- resolved from the `callisto@<version>` release tag, since a GitHub Actions `uses:` ref is split at the *last* `@` in the string, so the tag can't be written into `uses:` directly.
  
  `callisto-action` gains a `mode: version-pr | release` input (default `version-pr`, unchanged behavior); `mode: release` installs Callisto and runs `callisto release`, filling `published`/`publishedPackages` from its receipt. An invalid mode fails naming the accepted values. Its environment-setup step no longer references sibling action `setup-callisto` through `uses:` (`$/` and an external `owner/repo/path@ref` both resolve against whichever ref invoked the *top-level* workflow, not necessarily this action's own) -- it now calls `setup-callisto`'s install script directly by `github.action_path`, so it always uses its own checked-out ref, including when invoked from another repository's workflow.
- Workspace discovery no longer silently drops or misattaches packages: a manifest that fails to parse now fails with a clear error naming the path when it's a real workspace member, and warns otherwise; root detection asks git itself instead of hand-walking for .git, so GIT_DIR and worktrees are respected, and it reads [workspace]/[package] as real TOML tables instead of matching the text (a commented-out mention no longer counts); a platform package sitting beside its own Cargo.toml or pyproject.toml is never misattached to an unrelated owner; and a [[package]] rule that matches no packages gets a warning, matching the existing [[package-set]] one.
- Every add/version/schema/pre/release-pr error now carries its own code and help text instead of a generic message; release-pr decide/verify report the exact invalid-branch or duplicate-pull-request code instead of a plain string when reading a snapshot or decision from a file, inline JSON, or stdin.
- callisto init and callisto release --dry-run in a repository without an origin remote no longer print a stray git error about the missing remote.
- **`callisto init` asks for intent and previews the first release**
  
  `init` prints what it detected (ecosystems, packages, `origin`, last tags, binaries), then asks only for versioning mode and, when a package builds a binary, whether to ship binaries (product package, GitHub repository, target triples). It shows the resulting `callisto.toml` and the first release plan from the same derivation as `release --dry-run`, then writes after confirmation. Non-default tag conventions (`{name}-v{version}`, `{name}-{version}`, `v{version}`) are detected and kept. Non-interactive: `--yes --versioning <fixed|independent>`, plus `--artifact-target`, `--forge-repository`, `--product-package` to ship binaries. In a repository with no commit yet, `init` writes the config and skips the preview.
  
  Root discovery stays inside the Git repository: the nearest workspace manifest wins, otherwise the outermost package manifest (`Cargo.toml` with `[package]`, `package.json`, or `pyproject.toml`), so a single-package repository works for every command and `vendor/` or `node_modules/` packages never shadow it.
  
  Breaking:
  - `init` refuses an existing `callisto.toml`; the `[init]` reconcile flow is gone (an `[init]` table still loads and is ignored).
  - A non-terminal run needs `--yes --versioning`; `init --yes` alone errors.
  - `init` requires a Git repository with an `origin` remote.
  - Every command errors outside a Git repository (E058) and never searches above its root.
  - A forge repository other than `origin`'s GitHub repository is refused.
  - The written config holds only answers: no `[changesets]`, `[cascade]`, or `[init]` defaults.
  - `InitReport` JSON drops `diff` and adds `config`.
- Fix: a changeset with any unresolved package entry is no longer consumed, so its bump is not silently lost
- Text output now names registries (crates.io, npm, PyPI) and diagnostic severities in plain, lowercase words instead of internal identifiers
- Skip a private, versionless root package.json (standard npm/pnpm monorepo layout) instead of failing status/version with a missing-version error.
- callisto help <command> now prints that command's help text, including nested subcommands like callisto help pre enter
- init warns about each .changeset/config.json key it does not honour
- init rejects --forge-repository and --product-package up front when --artifact-target isn't given, instead of silently ignoring them
- An empty changeset (the shape `@changesets/cli add --empty` writes) is now accepted instead of rejected; status reports an entries-empty changeset as advisory instead of an error, and reports each unparseable changeset as a per-file error instead of aborting the whole command
- Install callisto with proto: `proto/callisto.toml` downloads the release binary for macOS arm64 and Linux x86_64 (glibc or musl). moon tasks run the installed CLI.
  
  Breaking:
  - Removed the moon extension (`callisto-moon` crate) and the `callisto-moon.wasm` release asset. Use the proto plugin and run `callisto` from moon tasks.
  - Removed the moon project-graph cross-check: `ProjectLocator::declared_edges`, `DeclaredEdge`, `DeclaredEdgeKind`, `DiagnosticCode::GraphEdgeDisagreement` and `LocateError::{MoonUnavailable, MoonOutputParse, IncompatibleMoonVersion}`. With it go `--strict-graph` on `status`, `version` and `snapshot`, `StrictFlag::StrictGraph`, and the `strict_graph` fields of `StatusOptions`/`VersionOptions`; `escalate` takes only `strict`.
  - Removed `IdentityResolver` and its errors `GraphError::UnsupportedIdentityEcosystem` (E155) and `GraphError::PackageIdentifierParse` (E156); the `callisto-cli` `wrapper` feature is gone.
- **Generated release workflows build split napi layouts**
  
  `callisto matrix` adds `manifestPath` to a napi target whose addon crate is outside the npm package's directory (e.g. `packages/napi` with the crate in `crates/binding`): the workspace `cdylib` depending on `napi` whose lib name equals `napi.binaryName`. The generated workflow passes it to `napi build --manifest-path`. No single match fails with E204, listing the candidates.
  
  The generated workflow also runs `npx --package @napi-rs/cli@3.10.4 napi ...`: the bare `npx @napi-rs/cli@3.10.4` form fails because the package has two bins.
- init's report now lists every file it wrote (or, under --dry-run, would write), including the generated workflow and .changeset/README.md
- **npm platform packages release with their owner**
  
  An `os`/`cpu` package listed in another package's `optionalDependencies` is now part of that owner: same version, no tag, pins kept in sync, published with `npm publish <dir>` before the owner. Covers napi addons and esbuild-style CLIs, including `npm/<platform>` dirs outside workspace globs. No `[[fixed-group]]` entry needed.
  
  Breaking:
  - Release intents are schema v4; re-plan intents made by older versions.
  - A dir with both `Cargo.toml` and `package.json` uses the `package.json` name as its npm release id (`npm/michi-node` → `npm/@orin-axi/michi`). Update `--package` selections and re-emit decision files.
  - Changesets must name the owner, not a platform package.
- `pre.json` and new changesets always live in the configured `[changesets] dir`, `pre exit` stages `pre.json`, and rewrites keep its line endings and indentation.
- In pre-release mode, `version` records the changesets it used in `pre.json`, so a rerun with no new changeset changes nothing instead of bumping the prerelease number and repeating changelog entries. Packages with ecosystem-qualified names (`cargo/foo`) also get correct changelog entries during a pre-release cycle.
- callisto add's interactive wizard prompts and preview now print to stderr, keeping stdout clean for scripts
- Add discoverable --help surface (8 user commands, plumbing hidden), NO_COLOR/CLICOLOR_FORCE/FORCE_COLOR-driven color and box-drawing tables for status and release --dry-run, and remove schema-version text from default output.
- **`publish-to` overrides keep `publishConfig`**
  
  A `publish-to = ["npm"]` override no longer drops `publishConfig.access` and `registry` from `package.json`, which made scoped packages publish as restricted. Explicit config still wins.
- **PyPI publish targets work in durable releases**
  
  Versions are checked via the PEP 691 JSON simple index instead of `pip`; yanked versions count as absent. Private indexes must serve the JSON simple index. Requires `curl` on the release runner.
- Package selectors (--package, add, matrix, release, config rules) now resolve consistently by any registered ecosystem-qualified or native manifest name; a mismatched-ecosystem rule or selector no longer silently matches the wrong package, and a package-set rule like `cargo/foo-*` now correctly restricts matches to Cargo packages instead of matching any ecosystem.
- **`callisto release` replaces the legacy publish commands**
  
  `callisto release` publishes, tags, and creates GitHub releases for every package whose current version has no tag, from any branch with a clean worktree. `--dry-run` previews the plan anywhere; `--package` restricts it and keeps fixed and linked group members together; an unreleased runtime, optional, or peer dependency must be selected with its dependent; `--receipt <file>` writes the receipt.
  
  Breaking:
  - Removed `callisto publish`, `plan-publish`, `tag` (including `--floating-major`), and `filter-plan`. Use `callisto release` or `callisto release --dry-run`.
  - Removed `schema --type tag` and `schema --type plan-publish`.
  - Removed graph `plan_publish`, `filter_plan_by_report`, `create_tags`, `toposort::publish_order`.
  - `release plan --package` now also keeps config-declared linked-group members, and both routes refuse an unselected unreleased dependency (`ReleaseSelectionInvalidReason::DependencyNotSelected`, was `PlatformDependencyNotSelected`).
  - Release decisions are written as schema 2 (adds `unreleasedVersion`). Schema 1 files still read; an earlier build cannot read a schema-2 decision.
- status --check and --strict now describe what they do in --help, instead of a vague placeholder
- Generated release actions install the version they're pinned to instead of always the newest release
- callisto completions and callisto schema no longer crash when piped into a command that closes its input early; they now exit cleanly.
- `init --artifact-target a,,b` (flag or interactive) errors naming the empty target instead of silently dropping it.
  The PyPI real-registry e2e test now builds offline (`--no-isolation` against the CI venv pinned setuptools/wheel); no network fetch during `python -m build`.
  `status --check` reports an ambiguous bare package name in a changeset as the `AmbiguousPackageName` diagnostic (exit 1) instead of hard-erroring.
- snapshot refreshes each ecosystem's lockfile too, so a locked install resolves right after it
- pre enter after pre exit resumes the cycle under the new tag instead of failing with a stale already-in-pre-release-mode error; pre exit with no pre.json now reports a clear fix instead of a raw I/O error
- init works without an origin remote: it writes the config and warns instead of refusing, skipping the workflow and shipping-binaries questions until origin is added
- **Faster failure on unreachable registries**
  
  Registry checks disable cargo's and npm's own retries and rely on Callisto's bounded retry. An unreachable registry now reports in seconds instead of 1–6 minutes.
- **Release artifacts are declared per package**
  
  Any targets and asset names are now accepted, not only Callisto's own four. Each artifact is recorded under the package that builds it; one whose package isn't in the release fails with E179. `--package` on one `[[fixed-group]]` member releases the whole group.
  
  Breaking: replace `[release] artifact-targets` with one block per target:
  
  ```toml
  [[release.artifact]]
  package = "cargo/my-cli"
  target = "aarch64-apple-darwin"
  asset-name = "my-cli-aarch64-apple-darwin.tar.gz"
  ```
- `callisto release` publishes a product's GitHub release only after every configured asset has uploaded, including assets built by other packages, and uploads use the product's prerelease flag. Before, such releases could publish incomplete or fail with E167.
- Release checks against crates.io, npm, PyPI, GitHub and the git remote retry on timeouts and connection failures, and a PyPI rate limit waits for `Retry-After`, instead of failing the release.
- **One release destination: `[release].forge-repository`**
  
  The forge destination moves to `forge-repository = "owner/repo"` directly under `[release]`. Each publish target uses its own registry key (`[registries.<key>]` or the built-in default). A legacy `[release.profiles.production].forge-repository` is still read when the new key is absent.
  
  Breaking:
  - `--profile` is removed from `release plan` and `release execute`.
  - `[release.profiles.<name>]` other than `production` is rejected; `registry-routes` is ignored.
  - Setting both `[release].forge-repository` and a different `[release.profiles.production].forge-repository` is rejected.
  - Release intents are schema v5 and run envelopes v3; re-plan intents made by older versions.
- **`release plan` checks what `publish` checks**
  
  Release planning now rejects an untrusted or non-https npm `publishConfig.registry`, requires an unreleased npm platform dependency to be selected with its owner, orders dev-only dependency cycles instead of failing, publishes scoped npm packages as public by default, and uses the changelog section as GitHub release notes (falling back to generated notes with a stderr notice).
  
  Breaking:
  - `--package` for a package with no pending release fails with "nothing pending to release"; one with no publish target fails with "no publish target"; a name not in the workspace is an unknown package.
  - `release plan` fails with E199 for a `publish-to` target it cannot dispatch (NuGet) instead of skipping it.
  - `callisto_graph::commands::registry_argv::npm_publish_directory_argv` takes the package name.
- The release PR action commits exactly the staged content and works when run from a subdirectory; a mismatch fails with E059.
- A release rerun adopts an existing annotated tag on the right commit even when its message differs, instead of failing depending on whether the tag had been fetched.
- **Rerunning a failed release completes it**
  
  `release execute` now adopts every effect that already landed, including a published registry version (one warning line each), and builds the receipt from in-memory execution state, so "Re-run failed jobs" finishes a partial release instead of failing with E174. Execution state is no longer persisted, and E173 and E174 are gone.
  
  Breaking:
  - `release execute --recovery` and `--state` are removed.
  - `release reconcile` and `callisto schema --type release-state` are removed.
  - The receipt's run envelope drops `kind` and is envelope schema v2.
- Output that explains why a package is released reports `release-trigger = changeset` as the default, not `auto`.
- **Release trust allows gitignored build output**
  
  A release no longer aborts because a gitignored path exists (built `.node` files, `dist/`, `node_modules/`). Modified tracked files and untracked files still block it, and the error names the path.
- Every CLI error now carries a numeric E#### code instead of a callisto::name string
- A malformed release intent file (duplicate operation, cycle, mismatched artifact roster, digest mismatch, or unsupported trust profile) reports its own specific error code from release verify and release execute, instead of one generic code for every case.
- **One build: every ecosystem and commit inference always compiled in**
  
  Commit inference now ships in the release binary. It runs only for packages with `release-trigger = "auto"`; the default `changeset` trigger is unchanged.
  
  Breaking:
  - Removed Cargo features `cargo`, `npm`, `inference` (`callisto-cli`, `callisto-graph`) and `cargo`, `npm`, `pypi`, `go`, `maven`, `nuget`, `deno` (`callisto-manifests`).
- `callisto init` now generates a release workflow for workspaces that ship `[[release.artifact]]` binaries or napi platform packages, instead of skipping them. The file is `.github/workflows/callisto-release.yml` with four jobs: `version-pr`, `plan` (`callisto release plan`), `build` (a matrix of the napi targets from `callisto matrix` plus one entry per artifact slot in the release intent) and `execute` (`napi artifacts`, then `callisto release artifact-manifest`, then `callisto release execute`). In npm workspaces, `execute` sets `NPM_CONFIG_PROVENANCE` so npm publishes carry provenance. Planning runs only when the pushed commit writes `.callisto/release-decision.json`, so ordinary pushes leave `plan`, `build` and `execute` skipped.
  
  Workspaces with maturin platform builds, or with platform packages and no `napi.targets`, still get no generated workflow. Passing `--workflow` for one of them fails with E202 and names the reason. Otherwise `init` skips the question and prints a note, which also appears in `InitReport.diagnostics` as `workflow-generation-unsupported`.
  
  `callisto matrix --format json` now lists each cargo `[[release.artifact]]` binary as a `cargo` entry in `platformTargets`, with the slot's asset name as its `artifactName`. A package that declares both platform targets and release artifacts fails with E203. A cargo `[[release.artifact]]` naming no workspace package is reported as an `unknown-package` warning instead of being dropped silently.
- The `callisto-format` and `callisto-vcs` crates are merged into `callisto-model`, and `callisto-conventional` and `callisto-changelog` into `callisto-graph`. The CLI is unchanged; library users import `callisto_model::format`, `callisto_model::vcs`, `callisto_graph::conventional` and `callisto_graph::changelog`.
- `callisto matrix` no longer collides a Cargo crate and an npm package that share a bare name (the common napi split layout).
  
  `--package` now accepts an ecosystem-qualified id (`cargo/foo`, `npm/foo`); a bare name resolves only when it names exactly one package, otherwise it errors listing the qualified candidates. A `[[release.artifact]]` binary now binds only to its own cargo package, never to a same-named package in another ecosystem.
  
  Breaking:
  - `platformTargets`/`runtimeVersions` map keys are each package's ecosystem-qualified id once a same-named package in another ecosystem exists, not always the bare name. Consumers indexing by bare name must switch to the qualified id (or keep working unchanged for any package whose name is unique in the workspace).
- callisto add --summary without --package on a non-terminal stdin now names --package as the missing flag, instead of claiming no flags were given
- **E180 for GitHub's tag-push workflow guard**
  
  When GitHub refuses a tag push from `GITHUB_TOKEN` (the tagged commit's workflows differ from every branch tip, e.g. when recovering an older release), `callisto release` reports E180 with the tag and the fix: push the tags with a PAT or deploy key, then re-run the release. Previously a generic E164.
- schema --type validate is removed and --type pre is renamed to --type pre-state (the .changeset/pre.json file shape, not pre's own report); an unrecognized --type value is now a usage error listing the supported values
- `version` checks `--strict` before writing anything, and refuses with E121 while an earlier run's changes are uncommitted and its changesets are still pending.
- `callisto status` shows real pending severity (cascade, fixed/linked groups) and folds in `validate`'s well-formedness checks under `--check`; `callisto validate` is removed.
  
  Breaking:
  - `callisto validate` is removed. Use `callisto status --check` and `callisto release --dry-run`.
  - `status --check` exits 0/1: a conventional gate on error-level diagnostics only, regardless of pending changesets. Use `status --format json`'s `.pending` (count of packages with a planned bump) to detect pending changesets.
- JSON reports (add, status, matrix, version, pre, snapshot, init, compose-pr-body) now include command and dryRun fields, and JSON error output includes command and path fields.

## 0.8.0

- **Recoverable, validated product releases**
  
  `callisto release` now recovers interrupted releases from provider observation (registry, remote tag, forge release, assets) and validates arguments and the receipt path before any effect. One profile authority governs execution (E198). `previous-tag-templates` keeps renamed tags discoverable, and the CLI product tag is now `callisto@{version}`. The `[release]` table configures product artifacts and a qualified `product-package`, with canonical destination isolation and per-command deadlines. Sources without a `[release]` plan get zero artifact slots and a notice. Registry observation goes through the ecosystem package manager, so registry resolution and authentication (including private registries) are handled by the same tool and configuration the publish uses: cargo is observed with `cargo info NAME@VERSION --registry REGISTRY` run from the workspace root, npm with `npm view`. A registry endpoint must now be `https`, with no loopback exception. PyPI publish targets are refused at plan time, because pip cannot distinguish a missing project from an unreachable index and so can never prove a version absent. A yanked crate version reads as absent (`cargo info` cannot see yanks) and the publish that follows fails closed with the typed unconfirmed-publication error rather than producing a receipt. Transient registry and forge failures (429, 5xx, rate-limited 403, timeouts, unreachable registries) are retried with bounded exponential backoff that honours `Retry-After`, instead of aborting the release. The GitHub Release is now created as a draft and published only after every product asset has been uploaded to it, so a release is never visible without its assets and an interrupted run cannot leave a published, incomplete release behind; release intents planned by an earlier version must be re-planned.

## 0.7.2

- Released together with the `workspace` fixed group.

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

