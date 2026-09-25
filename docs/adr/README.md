# Architecture decision records

- [1. Changesets-compatible change records](0001-changesets-compatible-change-records.md) — `.changeset/*.md` byte-compatible with `@changesets/cli`, commit inference opt-in; guardrail: do not make Conventional Commits the primary input or change the file format without a new ADR.
- [2. Merging the release PR is the only approval](0002-merge-is-the-only-release-approval.md) — a verified merge publishes automatically; guardrail: do not add a GitHub Environment, required reviewer or second approval without a new ADR.
- [3. The committed version decision is the release authority](0003-committed-decision-is-release-authority.md) — CI verifies the merged commit against `.callisto/release-decision.json`; guardrail: do not re-derive cascade or group policy after merge without a new ADR.
- [4. Reruns observe and adopt landed effects; no persisted execution state](0004-reruns-adopt-landed-effects.md) — every run observes providers and adopts what landed, state stays in memory; guardrail: do not add a persisted execution state file or recovery commands without a new ADR.
- [5. Ecosystem tools publish and own authentication](0005-ecosystem-tools-publish-and-own-auth.md) — `cargo`, `npm`, `twine` and `gh` publish with their own credentials; guardrail: do not add native registry clients or a credential pre-flight without a new ADR.
- [6. System git only](0006-system-git-only.md) — all Git access shells out to `git`; guardrail: do not add gix or a second Git backend without a new ADR.
- [7. One CLI distributed through proto](0007-one-cli-distributed-through-proto.md) — moon users run the CLI installed by proto; guardrail: do not add a moon WASM extension or WASM build without a new ADR.
- [8. Platform packages are owned through optionalDependencies and release with their owner](0008-platform-packages-owned-through-optional-dependencies.md) — an `os`+`cpu` package with one owner releases with that owner; guardrail: do not require fixed-group entries or explicit config for platform packages without a new ADR.
- [9. Format-preserving manifest writes gated by ApplyPermit](0009-format-preserving-writes-gated-by-apply-permit.md) — `toml_edit` and fingerprinted JSON through `atomic_write(&ApplyPermit)`; guardrail: do not use serde round-trips, regex edits or unpermitted writes without a new ADR.

## Rules

- An ADR records an architecturally significant decision that had a credible alternative.
- A changed decision gets a new ADR with `Status: Supersedes N`. The old file moves to `docs/adr/superseded/` with `Status: Superseded by M`. Agents do not load that folder.
- Every reason cites a source (commit, PR, historical doc). If none exists, write "Reason not recorded".
- Behaviour belongs in `docs/specs/`; coding rules belong in `AGENTS.md`.
