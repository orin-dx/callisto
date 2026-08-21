---
callisto-model: minor
callisto-graph: patch
callisto-vcs: patch
callisto-cli: patch
callisto-conventional: patch
---

**Security hardening across publish, git, and subprocess handling**

- **Breaking:** a package name starting with `-` is now reported as its own error, instead of being misreported as a path-traversal error.
- A malicious package name can no longer inject extra flags into the underlying `cargo publish`/`npm publish`/`pypi publish` command.
- A malicious `publishConfig.registry` URL can no longer redirect an npm publish to an unapproved registry.
- An absolute or `..`-containing `changesets.dir`/changelog path in `callisto.toml` is now rejected instead of allowing writes outside the workspace.
- Credentials no longer leak into error messages from failed git or registry-CLI commands.
- A runaway subprocess can no longer exhaust memory via unbounded output capture, or hang a command indefinitely.
- A tag name starting with `-` is now rejected, closing a git argument-injection hole.
