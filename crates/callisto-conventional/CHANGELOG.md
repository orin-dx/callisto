# callisto-conventional

## 0.6.0

- Released together with the `workspace` fixed group.

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

## 0.3.0

### Minor Changes

- Release update

