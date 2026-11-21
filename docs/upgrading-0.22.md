# Upgrading to 0.22

Do these in order when moving from 0.21.x to 0.22.0. Each step ends with how you know it worked. Run `callisto status --check` after the config steps, and `callisto release --dry-run` at the end; it prints `Nothing to release.` when every version is already published.

## Install

1. Install 0.22.0 locally, and move every pin of Callisto in your other workflows to 0.22.0: a `version:` input of `setup-callisto`, and the ref of `orin-dx/callisto/.github/actions/...` steps. The release workflow is regenerated in step 10. Worked: `callisto --version` prints `callisto 0.22.0`, and searching `.github/` for your old version finds nothing.

## Config

2. Rename `[registries.cratesIo]` and `[[package-set]]`. Both now fail at config load with E325.

   ```toml
   [registries.crates-io]   # was [registries.cratesIo]
   [[package]]              # was [[package-set]]
   ```

   Worked: `callisto status --check` loads the config.

3. Fix registry entries that used to load and then fail in a release.
   - Remove `nuget` from `publish-to` (E330).
   - Remove `kind` from `[registries.crates-io]`, `[registries.npm]` and `[registries.pypi]`, or give a custom registry another name (E334).
   - Rename a registry called `none` or `github-release`, or one with characters other than letters, digits, `-` and `_` (E325).

   Worked: no E325, E330 or E334 from `callisto status --check`.

4. Decide on peer escalation. `cascade.peer-escalation` now defaults to `false`: an out-of-range peer bump no longer releases the dependent as a major. To keep the old behavior:

   ```toml
   [cascade]
   peer-escalation = true
   ```

   Or add an explicit major changeset for the dependent. Worked: `callisto version --dry-run` shows the bump you expect.

5. Rename one of any two packages whose names a registry treats as equal, such as `My_Pkg` and `my-pkg`, or `foo-bar` and `foo_bar`. Worked: no E100 from `callisto status --check`.

6. Optional: shorten the release config. Replace the product's `[[release.artifact]]` blocks with one `targets` list and an `asset-name` template, and drop `forge-repository` (Callisto reads it from the `origin` GitHub remote). Keep `[[release.artifact]]` for another package's artifact or an irregular name such as a Windows `.zip`.

   ```toml
   [release]
   product-package = "cargo/tool"
   targets = ["aarch64-apple-darwin", "x86_64-unknown-linux-gnu"]
   asset-name = "tool-{target}.tar.gz"
   ```

   Worked: no E197 from `callisto status --check`, and with a pending release `callisto release --dry-run` lists the same `upload` asset names as before.

7. Registry settings for yarn and bun workspaces. `npm publish <tarball>` ignores `.yarnrc.yml` and `bunfig.toml`, so a registry set there now fails with E168. Move it to `publishConfig.registry` in `package.json` or to `.npmrc`, and keep the token in `NPM_TOKEN`.

   ```json
   { "publishConfig": { "registry": "https://registry.example.com/" } }
   ```

   Worked: no E168 from `callisto release --dry-run`.

## Scripts

8. Drop `--forge-repository` from scripted `callisto init` calls, and `--refresh-lockfiles` from `callisto version` calls (lockfiles refresh by default; use `--no-refresh-lockfiles` to skip). Worked: the scripts run without a usage error.

9. Match `E309` where scripts matched `E216`, `E217`, `E218` or `E282`, and match `package-rule-matched-nothing` where they read `package-set-matched-nothing` from `--json`. In `init` runs, a scoped npm package with no `publishConfig.access` is refused with E342 when `access` is `restricted` or absent: set `publishConfig.access` in each listed `package.json`, or pass `--yes` to publish it as public. Worked: `callisto release plan --dry-run --package <ecosystem/name> --out plan.json` exits 2 with `E309`, and `callisto init` completes.

## Workflow

10. Keep a copy of `.github/workflows/callisto-release.yml`, delete it, and regenerate it. Then re-apply your own changes from the copy (extra steps, triggers, secrets), check that `on.push.branches` names your release branch (it is read from `origin/HEAD`), and close any stray release PR the old workflow opened.

    ```bash
    cp .github/workflows/callisto-release.yml callisto-release.old.yml
    rm .github/workflows/callisto-release.yml
    callisto init --workflow
    diff callisto-release.old.yml .github/workflows/callisto-release.yml
    ```

    `init --workflow` writes only the workflow when `callisto.toml` exists. Worked: the new file has a credential-free job per ecosystem you publish (`package-cargo`, `pack-npm`, `build-python`), and only `execute` references `secrets.`.

11. Release by a push that adds or changes the release decision file. The simple workflow no longer publishes just because an untagged version was merged. Worked: merging an unrelated change publishes nothing.

## Publish hooks

12. npm publishes now run `npm publish <tarball> --ignore-scripts`, so `prepublishOnly`, `publish` and `postpublish` no longer run. `prepack` and `prepare` run in the pack job, which has no registry credentials. Move work that must happen at publish time into `prepack`, or into a step of your own after `execute`. Worked: `callisto release --dry-run` shows the work happening where you moved it.

13. Run `callisto release plan` again. A release plan saved by an earlier version is refused with E215. Worked: `callisto release execute` accepts the new plan.
