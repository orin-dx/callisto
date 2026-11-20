# Releasing

How a Callisto-managed release moves from changeset to published package.

For registry credential setup (npm, crates.io, PyPI), see [`publishing.md`](publishing.md).

---

## Local release: `callisto release`

`callisto release` selects packages without a tag for their current version. It publishes to registries, creates tags and GitHub releases, and records HEAD as the source. It can run on any branch.

- `callisto release --dry-run` prints the plan and performs no effect. It works anywhere, including on a dirty worktree or without an `origin` remote (tags are then noted as unbound).
- `--package <ecosystem/name>` (repeatable) restricts the run to named unreleased packages, all distributions of each (naming `npm/dual` also releases `cargo/dual`), plus every member of their fixed or linked groups. An unreleased workspace package that a selected one depends on (runtime, optional, or peer) must be selected too.
- It refuses a dirty worktree: a tracked modification or an untracked file not covered by `.gitignore`. Ignored files never count.
- It requires the git repository root to be the workspace root; a workspace in a subdirectory fails with `E170`.
- It prints `Nothing to release.` and exits 0 when every package is already tagged.
- It prints a summary. With `--receipt <file>` it also writes the receipt there, only when every selected operation succeeded. An existing tag can exclude unfinished work before those operations are selected; see the retry limit below.
- A workspace with artifact slots or napi platform packages must release through CI. See the generated workflow below.

`cargo`, `npm`, `twine`, and `gh` report their own authentication failures. With the original release intent, the executor can adopt matching completed effects.

A local release refuses with E320, before any effect, when the registry already holds a version it would release but the package's files changed since the commit that set that version, or no commit in this history sets it. Check out the version commit the error names and run `callisto release` again: it adopts the published version and tags that commit. When no commit is named, check out the commit that was published, or tag it by hand.

### Current retry limit

If a local release tags a version but fails before pushing the tag or completing GitHub work, a fresh `callisto release` at the same commit lists the package again, observes each remaining effect and finishes it.

- `Nothing to release.` is not proof of completion for a tag on an older commit: the retry looks only at tags on the commit being released.
- CI releases based on the merged release PR retain the original package list.

---

## Generated workflows

`callisto init --workflow` (or answering yes to its prompt) writes `.github/workflows/callisto-release.yml`; in a workspace that already has `callisto.toml` it writes only the workflow. Keep that filename for generated artifact releases.

Init never overwrites an existing file. This repo's [release workflow](../.github/workflows/callisto-release.yml) is a worked example.

- **Simple shape** (no artifact slots or napi platform packages): `version-pr` maintains the release PR. On each default-branch push, `package` (no secrets) checks whether the commit adds or modifies the release decision, and if so plans the release, builds Python distributions and packs npm packages, and hands them to `execute`, which publishes exactly those files. A push that merely contains an untagged version publishes nothing; recovery is "Re-run failed jobs".
- **Build-matrix shape** (artifact slots or napi platform packages): a release PR, then plan, build (read-only), attest (no checkout, no commands), optional Python build, crate packaging and npm pack jobs (no credentials), and execute. The build covers configured targets and artifact slots.
- Maturin builds, and platform packages without `napi.targets`, get no generated workflow.

Generated actions configure npm and Twine authentication during execution. Manual workflows must configure native authentication as described in [publishing.md](publishing.md).

Generated build support has these boundaries:

- Cargo `bin` targets can become GitHub release assets. npm `bin` and Python `[project.scripts]` are registry releases, not artifact products (E304).
- Co-located Cargo/Maturin manifests fail discovery with E007.
- Custom artifact builders need `--no-workflow` and a manual workflow that fills every configured artifact slot.
- Registry-only npm and PyPI releases need no artifact slots.

The default branch is the approval boundary: a merge to it publishes. Protect it with a rule that requires pull requests and reviews (GitHub -> Settings -> Rules). Init prints this reminder after writing the file.

Pin the Callisto actions by commit with the version in a comment, such as `uses: orin-dx/callisto/.github/actions/callisto-action@<sha> # callisto@1.0.0`. The action installs the release that commit belongs to. From 1.0.0 the floating tag `v1` also works (`@v1`): each stable 1.x release moves it to its release commit, never backwards, and the action installs that release. A mutable tag is only as safe as the repository that moves it, so a pinned commit stays the stricter choice.

Build-matrix limits:

- An addon crate outside the napi package directory needs a unique crate whose lib name matches `napi.binaryName` (E204).
- Generated `.node` files must be gitignored; release execution requires a clean checkout.
- Cargo `bin` targets build with `cargo build --release --locked`. Other products, such as `cdylib`s, need a manual workflow.
- The napi build and placement roster comes from the release intent, so an unrelated addon does not block a release of another package.
- npm lifecycle hooks split at the pack. `prepack` and `prepare` run in the job that packs the package, which holds no registry credentials. The publish is `npm publish <tarball> --ignore-scripts`, so `prepublishOnly`, `publish` and `postpublish` no longer run. Move work that must happen before a release into `prepack`, and anything that acts on the registry into a step of your own after `execute`. Effects an owner-defined hook still has at pack time are outside Callisto's release graph.

---

## Release PR and approval

The generated workflow keeps a release PR current. Merging that PR to the protected default branch authorizes publication; Callisto adds no approval step. Require reviews and CI checks in branch protection. A CODEOWNERS file alone does not enforce review.

During a pre cycle (`callisto pre enter <tag>`) the release PR works the same way: each new changeset or inferred commit opens a PR for the next prerelease, a cycle with nothing new opens none, and after `callisto pre exit` the next PR releases the stable versions.

The action's `version-pr` mode updates the PR; `release` runs `callisto release` for a hand-written local workflow, and `execute` runs `callisto release execute` on the intent and files a credential-free job prepared. Generated workflows use plan/build/execute (build-matrix) or package/execute (simple). See the [action inputs](../.github/actions/callisto-action/action.yml).

Only `execute` receives registry credentials or an OIDC token for publishing. Keep `CARGO_REGISTRY_TOKEN`, `NPM_TOKEN`, and `TWINE_PASSWORD` out of workflow scope, planning, build, pack and attestation jobs, and action inputs. The attestation job holds `id-token: write`, so it has no checkout and runs no command.

After a failed committed-decision CI release, "Re-run failed jobs" adopts completed effects and performs the rest. A green workflow alone is not proof of publication: check the receipt and providers.

## PR pre-flight verification

[`.github/actions/callisto-validate/action.yml`](../.github/actions/callisto-validate/action.yml) is a reusable action for your PR workflow:

- `callisto status --check` — one gate covering config health, package discovery, and changeset syntax: exit 0 with no error-level diagnostics, exit 1 otherwise.
- `callisto status --check --since origin/main` — also fails when the pull request changes a package that no changeset names. A change that needs no release takes a changeset with severity `none`.
- `callisto release --dry-run --format text` — simulates the release plan without effect.
- Writes a workspace status summary to the job's GitHub Actions summary page.

---

## Release identity and recovery

Every release run records two different revisions:

- The **orchestration revision**: the workflow and Callisto CLI that coordinate the run.
- The **release-source revision**: the merged release commit whose exact versions are released.

A recovery is a new run using current orchestration and the merged release PR's full source SHA. It does not rerun historical workflow code or invent a newer version. Callisto's own workflow asks [`callisto release pending`](#finding-releases-that-did-not-finish) for the oldest unfinished release on each main push. It verifies that SHA against the merged managed PR and the current main history before planning. After a release executes, the workflow dispatches the next unfinished one, or refreshes the release PR for newer changesets when none remains.

To retry a failed run for the same source, use "Re-run failed jobs" or dispatch the workflow on main with `release_source_sha=<merged PR merge SHA>`. Matching completed effects are adopted; conflicting or uncertain results stop the run. A receipt is written only after every selected operation succeeds. The merge SHA is available in the PR's merge commit and does not depend on a proposed version tag. Release runs execute one at a time: a push during a release waits, then finds nothing pending. A dispatch with an explicit SHA queued behind a push can be replaced by it, so dispatch it again if it did not start.

A failed release is selected again on every main push until it finishes, which also holds back the release PR refresh. To get past one you finished by hand, push its missing tags, or run `callisto release pending --from <newer release sha>`. Otherwise recover it with a dispatch of its SHA (see [recovering an older release](#recovering-an-older-release)). Finish an older unpublished release before merging a newer release PR: the newer one publishes without any warning that the older version was never tagged.

Callisto waits for a published version to appear before it announces anything: up to 6 minutes for npm and PyPI, 2 for Cargo. npm packages are published back to back and waited for together, so a release of many npm packages waits once, not once per package; Cargo and PyPI publishes are waited for as they are published. If the registry is slower it stops with E157; re-run the failed job once the version shows, or set `confirm-timeout` on the registry in `callisto.toml` (see [configuration](config.md#registries)) and keep the job's `timeout-minutes` above the total wait.

## Finding releases that did not finish

`callisto release pending` lists the merged release commits whose release did not finish, oldest first, with what is missing. It reads each release commit's committed decision and checks that every package has its tag and every GitHub release is published, so it needs no checkout of the old commit and publishes nothing. A release publishes to the registries before it tags, so a missing tag means the release stopped early.

```sh
callisto release pending                    # text
callisto --format json release pending      # for a workflow
callisto release pending --from <sha>       # skip an older release that cannot finish
```

Recover each one with the dispatch below, oldest first.

## A stale release PR

Callisto's workflow rebuilds the release PR on every push to `main`. While that rebuild is pending the PR is stale: if changesets changed on `main` since it was built, merging it releases the wrong set. So a second workflow, `Hold release PR`, turns the open release PR into a draft the moment `main` moves, and the release workflow marks it ready only after a rebuild from the new tip. If the rebuild fails, the PR stays a draft and cannot be merged. Re-run that `Release` workflow run, and the PR becomes ready when it succeeds. A transient GitHub error during the rebuild is attempted up to four times in all before it fails.

If a stale release PR whose changesets were already consumed on `main` was merged anyway, the release fails with [E165](errors.md) because the changesets the merge deleted are not the ones its decision recorded, and every later push to `main` retries it. A stale PR that deletes no changesets, one that releases only bumps inferred from commits or a prerelease PR of a Changesets v2 pre cycle, is not caught that way: E165 still refuses it when `main` already holds its versions, and `Hold release PR` keeps a stale PR in draft. Nothing was published. Revert the merge's release payload (the version bumps, changelog entries and `.callisto/release-decision.json`, not the changeset files) in a pull request to `main`. The next push rebuilds the release PR from the changesets that are really pending.

## Recovering an older release

Recovering a commit that is no longer a branch tip may require these manual actions:

**E180: the workflow can't push tags.** GitHub rejects a `GITHUB_TOKEN` tag push when the tagged commit's workflows differ from every branch tip. Push every tag of the release yourself (PAT or deploy key), then dispatch the release for that SHA:

```sh
SHA=<release-source sha>
git tag -a "<tag>" -m "Release <tag>" "$SHA"   # each tag of the release
git push origin "<tag>"
gh workflow run callisto-release.yml --ref main -f release_source_sha="$SHA"
```

**`ArtifactAssetDiffers`: rebuilt assets don't match.** Rebuilt tarballs aren't byte-identical. Delete the mismatched assets and dispatch again:

```sh
gh release delete-asset "<release tag>" "<asset>" --yes   # each mismatched asset
gh workflow run callisto-release.yml --ref main -f release_source_sha="$SHA"
```

## Publication checks

Callisto checks the registry, remote tag, and GitHub release before each effect. An exact match is adopted; a conflict or uncertain result stops the run. Check the receipt and providers before treating a release as complete.

Registry checks confirm version presence, not identical bytes or the npm dist-tag. A Python release is complete only when every built wheel and sdist is on the index; a partly uploaded one is finished by uploading the missing files, and a sealed build also compares each file's digest. See [publishing limits](publishing.md#current-python-support-limits).

GitHub Releases start as drafts. Assets upload before the release is made public.

---

## Platform-package publish order

`callisto release execute` follows the derived operation DAG, not a fixed global Cargo -> npm -> PyPI pipeline. Native dependency edges and attached npm platform prerequisites constrain that order. Platform packages publish before their npm owner.

A napi-rs distribution can publish N platform-specific npm packages plus one owner package whose `optionalDependencies` pin their versions. Maturin instead produces Python wheels and optionally an sdist under one PyPI distribution/version; those are files, not npm platform packages.

- **A platform package fails**: its owner's main package is never attempted, so no `optionalDependencies` point at a version that was never uploaded. Fix the cause and re-run — it adopts every effect that already landed.
- **A platform dependency is missing from the selection entirely**: planning refuses (`ReleaseSelectionInvalid`, "select it too") before anything is published.

For a package with both Cargo and npm distributions, one registry publish can still lag behind its shared tag or public GitHub release. Do not treat that announcement as proof that both distributions are available.
