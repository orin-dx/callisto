---
callisto-model: minor
callisto-graph: patch
callisto-vcs: patch
callisto-cli: patch
callisto-conventional: patch
---

# Security hardening across publish, git, and subprocess handling

## Breaking Changes

- **New `PackageIdParseError::LeadingHyphen` variant**, split out from `PathTraversal` (which was wrongly reported for inputs with no `..` at all).

## Security

- Package names are now validated before hitting `cargo publish`/`npm publish`/`pypi publish`, closing an argument-injection hole.
- `publishConfig.registry` for npm packages is now checked against an allowlist, blocking SSRF via a malicious registry URL.
- The `changesets.dir`/changelog path config value is now rejected if absolute or containing `..`, closing a path-traversal hole.
- Credentials are now redacted from git and registry-CLI stderr before it lands in error messages.
- Subprocess stdout/stderr capture is now bounded, so a runaway child process can't exhaust memory (DoS); reader-thread wait is also bounded so a hung descendant can't block `run_with_timeout`.
- Tag names with a leading hyphen are now rejected, and shelled git refs are qualified, closing a git argument-injection hole.
