# Security Policy

## Reporting a Vulnerability

Email **security@orin-dx.com** — don't open a public issue for anything that could be exploited before a fix ships.

Include:
- Which crate or code path is affected
- The concrete failure scenario — what an attacker could do, and how
- Steps to reproduce, if you have them

Expect an acknowledgment within 5 business days. We'll keep you posted as a fix moves through triage, and credit you in the release notes unless you'd rather stay anonymous.

## Scope

Callisto edits manifests and pushes releases across multiple package registries from inside CI, so the threat model is higher-stakes than a typical CLI tool:

- **Registry token exposure** — Callisto's own release workflow (`.github/workflows/callisto-release.yml`) runs with `contents: write`, `pull-requests: write`, and holds `CARGO_REGISTRY_TOKEN`/`NPM_TOKEN` secrets. A way to make Callisto (or a workflow using `orin-dx/callisto-action`) exfiltrate those tokens, or publish to a package other than the one it was asked to, is a critical finding.
- **Workflow/action injection** — Callisto is designed to run unattended on `push` to `main` in downstream repos. An untrusted PR that could get its content executed by the release workflow (rather than merely analyzed) is in scope.
- **Manifest parser safety** — `callisto-manifests` edits `Cargo.toml`/`package.json` via CST-preserving parsers (`toml_edit`, `serde_json`) rather than regex specifically to avoid corruption; a crafted manifest that causes incorrect writes, path traversal, or a crash mid-write (bypassing the atomic `NamedTempFile` + `fs::rename` guarantee) is a real finding.
- **Version/changelog spoofing** — a way to make `callisto version` compute or apply an incorrect bump, or inject arbitrary content into a generated changelog, that a maintainer could plausibly merge without noticing.
- **Git operation safety** — `callisto-vcs` performs native in-process Git operations via `gix`; unsafe handling of an untrusted repository (submodules, refs, hooks) is in scope.

Dependency vulnerabilities are already checked continuously via `just audit` (`cargo deny check advisories`) in CI — a report that only restates an existing advisory `cargo audit`/`cargo deny` already flags isn't a new finding, but a way to bypass or disable that check is.

Out of scope: vulnerabilities in Cargo, npm, PyPI, or GitHub Actions themselves — report those to the respective platform.

## Supported Versions

Security fixes land on `main` and the latest published release of each affected crate. Given Callisto is pre-1.0 (`0.3.3`), older minor versions are not backported — upgrade to the latest release.
