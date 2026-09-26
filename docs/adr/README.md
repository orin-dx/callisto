# Architecture decision records

- [1. Changesets-compatible change records](0001-changesets-compatible-change-records.md) — `.changeset/*.md` in `@changesets/cli`'s format, commit inference opt-in; guardrail: do not make Conventional Commits the primary input or change the file format without a new ADR.
- [2. The committed version decision is the release authority](0002-committed-decision-is-release-authority.md) — CI verifies the merged commit against the decision `callisto version --emit-decision` committed; guardrail: do not re-derive cascade or group policy after merge without a new ADR.
- [3. Reruns observe and adopt landed effects; no persisted execution state](0003-reruns-adopt-landed-effects.md) — every run observes providers and adopts what landed, state stays in memory; guardrail: do not add a persisted execution state file or recovery commands without a new ADR.
- [4. Ecosystem tools publish and own authentication](0004-ecosystem-tools-publish-and-own-auth.md) — `cargo`, the npm-family tools, `twine` and `gh` publish with their own credentials; guardrail: do not add native registry clients or a credential pre-flight without a new ADR.
- [5. System git only](0005-system-git-only.md) — every Git command in the `callisto` binary runs the system `git`; guardrail: do not add gix or a second Git backend without a new ADR.
- [6. moon integrates through a proto plugin, not a WASM extension](0006-moon-through-proto-plugin-not-wasm-extension.md) — moon users install the native CLI with a proto TOML plugin; guardrail: do not add a moon WASM extension or WASM build without a new ADR.
- [7. Platform packages are owned through optionalDependencies and release with their owner](0007-platform-packages-owned-through-optional-dependencies.md) — an `os`+`cpu` package with one owner releases with that owner; guardrail: do not require fixed-group entries or explicit config for platform packages without a new ADR.
- [8. Format-preserving manifest writes gated by ApplyPermit](0008-format-preserving-writes-gated-by-apply-permit.md) — `toml_edit` and fingerprinted JSON through `atomic_write(&ApplyPermit)`; guardrail: do not use typed serde round-trips for TOML, unfingerprinted `package.json` rewrites, regex edits or unpermitted writes without a new ADR.
- [9. A tag's identity is its commit and being annotated](0009-tag-identity-is-commit-and-annotation.md) — an existing annotated tag on the planned commit is adopted whatever its message; guardrail: do not compare annotation text or accept lightweight tags without a new ADR.
- [10. The workspace resolves natively after `version` or `snapshot`](0010-native-resolution-after-version.md) — a co-released dependent's spec is always raised to the new version and lockfiles refresh by default; guardrail: do not go back to rewriting only out-of-range specs or make lockfile refresh opt-in without a new ADR.
- [11. Packages are keyed internally by directory, not by their primary name](0011-packages-keyed-by-directory.md) — each directory's package carries one or more `(ecosystem, native name)` names, resolved through one `IdentityIndex`; promotion to `eco/name` is display-only; guardrail: do not key identity by primary manifest name or resolve selectors by exact `PackageId` comparison without a new ADR.

## Rules

- An ADR records an architecturally significant decision that had a credible alternative.
- `Proposed` until the PR that implements it merges, then `Accepted`.
- A changed decision gets a new ADR with `Status: Supersedes N`. The old file moves to `docs/adr/superseded/` with `Status: Superseded by M`. Agents do not load that folder.
- Every reason cites a source (commit, PR, historical doc). If none exists, write "Reason not recorded".
- Behaviour belongs in `docs/specs/`; coding rules belong in `AGENTS.md`.
