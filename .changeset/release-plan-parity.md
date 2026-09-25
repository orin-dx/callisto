---
callisto-cli: minor
---

**`release plan` checks what `publish` checks**

Release planning now rejects an untrusted or non-https npm `publishConfig.registry`, requires an unreleased npm platform dependency to be selected with its owner, orders dev-only dependency cycles instead of failing, publishes scoped npm packages as public by default, and uses the changelog section as GitHub release notes (falling back to generated notes with a stderr notice).

Breaking:
- `--package` for a package with no pending release fails with "nothing pending to release"; one with no publish target fails with "no publish target"; a name not in the workspace is an unknown package.
- `release plan` fails with E199 for a `publish-to` target it cannot dispatch (NuGet) instead of skipping it.
- `callisto_graph::commands::registry_argv::npm_publish_directory_argv` takes the package name.
