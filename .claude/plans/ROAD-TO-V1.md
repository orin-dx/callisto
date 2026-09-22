# Road to v1 — artifact model, config DX, and open defects

Written 2026-09-21, after the 0.8.0 self-release. Supersedes nothing; complements ACTIVE.md.

## Why this exists

0.8.0 published successfully (9 crates, 9 tags, GitHub release, clean receipt) after three failed attempts. The failures and the audit that followed exposed a set of design problems that are cheap to fix at 0.8.0 and expensive after 1.0. This records them with evidence so the reasoning is not re-derived.

---

## 1. Verified platform constraint: GitHub App tag-push guard

`GITHUB_TOKEN` is a GitHub App token. GitHub refuses any ref push -- **and Git Data API ref creation** -- where the ref's tree `.github/workflows/` differs from the tip of *any* branch.

Tested three ways during the 0.8.0 release:

| route | result |
|---|---|
| `git push`, malformed tag object | rejected (workflows message) |
| `git push`, well-formed annotated tag | rejected, identical message |
| `POST /git/refs` with `contents: write` | 403 Resource not accessible by integration |
| `POST /git/refs` creating a *branch* at that commit | 403 |

There is no `workflows` scope in a job's `permissions:` block (verified against GitHub's expressions/permissions docs), so this cannot be granted away.

**Consequence:** automated tagging works on the normal path (tagging the tip, as changesets and knope do) and can never work for recovery of an older commit whose workflows have since changed. Escape hatch: push the tags with a non-App credential (a PAT or deploy key).

**Not a ruleset.** Ruleset 22292852 is branch-target, `refs/heads/main` only, no tag rules.

## 2. Artifact model: the (a)/(b) fork

`PRODUCT_ARTIFACT_TARGETS` in `config/resolve.rs` was a compiled-in table mapping exactly four targets to asset names prefixed `callisto-`, with validation rejecting any other set -- and `resolve_product_release` then **discarded the configured value and returned the const regardless**. No other workspace could use `[release]`.

The model and the docs disagree about what an artifact slot is:

- Built for **(b) real slot identity**: `ArtifactSlotId.package` is non-optional (`release.rs:1262`); `validate_artifact_slot` (`:1085-1092`) ties the upload operation's package to the slot's; `ArtifactSlotOutsideDecision` (`:1578-1584`) checks the slot against the **decision roster**, not against the product.
- Written for **(a) build-source pointer**: SRL-10's "the product-level GitHub Release", singular; `raw.rs:27-29`; config nests artifacts under one `product-package`.

**Decision: (b).** Wire semantics change, cheap at 0.8.0, and (a) means permanently carrying machinery that does nothing.

**(b)'s DX cost is part of (b), not follow-up:** `derive_selected_release_decision` (`release_decision.rs:119-129`) retains `LinkedGroup` siblings but **not fixed-group** siblings, so `--package cargo/callisto-cli` would drop `callisto-moon` and fail with `ArtifactSlotOutsideDecision`. Either pull artifact-owning packages into the narrowed decision, or error with: *selecting X excludes Y, which produces asset Z*.

## 3. `callisto-moon.wasm` is a public contract

Referenced only in `.github/`, never in `.moon/` or `docs/` -- external moon workspaces pin the release-asset URL in their `workspace.yml`. The name cannot be derived away or renamed. **Undocumented**: no install path in `docs/`, and no version-pinning convention.

## 4. Semantics settled

- `publish = false` removes a **publish target**. It does not remove graph membership or versioning. Proven by 0.8.0: `callisto-fixtures` is `publish = false`, was bumped to 0.8.0 in the decision, and produced 9 (not 10) registry publishes.
- nx's "exclude `private: true`" rule is therefore **not** a valid analogue; adopting it verbatim would drop `callisto-fixtures` from versioning and break the fixed group.
- Discovery (all workspace members), publishing (`publish = false`), and versioning (independent vs `[[fixed-group]]`) are three separate questions.

---

## Pre-1.0 — breaking or contract-setting, must land now

1. **Design (b) slot identity**, including the selection-narrowing fix and its error message.
2. **Artifact config shape** -- derived names + `[[release.artifact]]`; delete `PRODUCT_ARTIFACT_TARGETS`.
3. **Archive extension**: `.tar.gz` (keep; more universally decompressible than cargo-dist's `.tar.xz`) -- freeze deliberately, it becomes installer API at 1.0.
4. **Windows naming rule** decided now even though unsupported -- `.zip` + `.exe` -- so adding a target later is not a breaking change to the naming contract.
5. **Default tag template** if it becomes derived from fixed/independent mode.
6. **Document the moon plugin install path and pinning convention.**

### Blockers for 1 and 2

- Repoint `.github/tests/verify-release-workflow-policy.sh:418-461` (`table_agreement`) off the Rust const **before** deleting it, or five hand-copied tables lose their drift check. Rewrite `crates/callisto-graph/tests/release_target_table_agreement_tests.rs`.
- `cargo_bin_name` must move out of `callisto-graph` and behind the `Manifest` trait (`ARCHITECTURE.md:321-346`); `Ecosystem` is pure data-only functions and must stay so.
- Settle the "artifact" vocabulary collision -- `docs/00-design.md:673` uses the word for an independently released package, the opposite sense.

## Post-1.0 — additive, safe to follow

Effective-config explain surface; typed diagnostic for the tag-push guard; `.sha256` sidecars; fixed-group glob members (literal names are already valid globs, so backward compatible); auto-include README/LICENSE/CHANGELOG in archives by discovery (cargo-dist pattern).

## Agreed cuts (endorsed, not yet done)

- Demote recovery-of-an-old-commit from automation to a documented assisted procedure. It is blocked by the platform guard **and** by `tar -czf` not being byte-reproducible, so `observe_artifact_upload` reads rebuilt assets as `ArtifactAssetDiffers`. **Unblock without code:** delete the mismatched assets, then re-dispatch -- `artifact.rs:153` returns `Absent` with no asset, and `release_execution.rs:159` maps `Absent => Ok(())`. **Do NOT** apply the obvious reproducible-tar fix: `tar --sort=name --mtime=@0 --owner=0 --group=0 --numeric-owner` is GNU-only and fails on the macos-14 bsdtar leg (verified: `tar: Option --sort=name is not supported`).
- Remove the post-publish receipt pass (`release.rs:385-395`): 24 fresh observations after everything is irreversible, any one of which reds a successful release.
- Correct two policy rules asserting false platform behaviour: the receipt `run_attempt` rationale (run 34702634375 uploaded the same fixed artifact name successfully in attempts 1, 2 and 3 -- artifact names are scoped per attempt, there is no collision), and the `always()` mandate at `verify-release-workflow-policy.sh:361-363`, which demands the literal string though its own comment says "a status function" and `!cancelled()` qualifies.

## Open defects not yet fixed

- `always()` on `execute` means Cancel does not stop the publish; `!cancelled()` is the right guard but the policy script rejects it. The two must land together.
- Policy permission rule (`:301-307`) checks only contents/id-token/attestations/write-all; `packages: write` or `actions: write` on `plan` passes clean.
- `Indeterminate` observations discard stderr (`registry.rs:338`) and `run_quiet` suppresses the live echo -- deliberate `CARGO_REGISTRY_TOKEN` redaction (`runner.rs:934`), so the fix is redacted stderr into the closed `ProviderIndeterminateCause` enum, not live streaming.
- Release-trust allowlist (`callisto-vcs/src/shell.rs:35-44`) has 8 entries against 19 `.gitignore` patterns; `*.wasm`, `callisto-schema.json`, `.codex/`, `.playwright-mcp/` and others would hard-abort a release mid-flight.
- `Cargo.toml:6` `[workspace.package] version = "0.3.3"` is dead -- no crate uses `version.workspace`.
- Stale remote branches `callisto/version-packages--<sha>`.
- Unpublished 0.6.0-0.7.2: four version PRs merged, never published; their changelogs describe versions nobody can install.

---

## DX principles (constrain all of the above)

1. **Nothing irreversible is inferred.** Where bytes go, what is published, what version -- explicit or derived from explicit state. Inference locates things; it never decides them. Specifically: do **not** infer `forge-repository` from the git remote. A fork or second remote would publish a release to the wrong repository.
2. **Infer facts, declare intent.** A fact has one correct answer discoverable by reading the repo (git remote, `[[bin]]` name, which packages exist, `.changeset`). Intent has several valid answers and no amount of reading reveals which (fixed vs independent, which package is the product, which targets to ship).
3. **Every inference is printable before it acts.** `plan`/`inspect` preview the release; the missing half is an effective-config view. `ConfigProvenance` and the `governed by <key> = <value>` rendering already exist -- promote them to a first-class surface **before** removing config keys, so the config never becomes less legible.
4. **Ambiguity is an error listing candidates, never a pick.**
5. **Derived names are stable across tool versions.** A Callisto upgrade must never silently rename a user's release assets and break their installer.
6. **Errors name the fix, not the invariant.** `E164` plus raw git stderr cost hours during 0.8.0; one sentence about the App workflow guard would have cost minutes.

### Interactive setup (`callisto init`) -- direction

**`init` writes intent, never facts.** This is what keeps interactive setup and zero-config coherent: a config file full of inferred values is drift waiting to happen, and every value written is a value that can go stale.

- Detect and report: ecosystem, workspace members, git remote, existing tags/versions, whether any package produces a binary.
- Ask only about intent: fixed vs independent, product package (only if binaries exist), artifact targets, tag convention.
- Write only the answers. Everything inferable stays absent from the file.
- Finish with a dry preview -- *"10 crates, none released; a first release would do X"* -- before writing anything.
- Non-interactive flag for CI and scaffolding.

Callisto has a real advantage worth protecting here: it reads current versions from manifests, so unlike release-please it needs no pre-seeded version-state file and no `bootstrap-sha` on first run.

### Cascade defaults are already redundant

`[cascade]`'s four declared values are all the defaults (`resolve.rs:499, 513, 520, 525`), as is `[changesets] dir = ".changeset"` (`:480`). `registry-routes = { cratesIo = "cratesIo" }` is an identity mapping. Removing them shrinks the file with zero behaviour change -- but only after principle 3's explain surface exists.

---

## 5. Target resolution must not be Rust-shaped (added after the artifact work began)

Two systems for the same problem already exist in-repo:

- `matrix.rs:12-34` `triple_host_runner_use_cross` -- an **18-triple** table giving host runner and whether `cross` is needed, plus generic artifactName derivation, built for the napi/npm platform-package path.
- `config/resolve.rs` `PRODUCT_ARTIFACT_TARGETS` -- a **4-entry** table with Callisto's own asset names, built for the product-release path, ignoring the above.

Distinguish two kinds of hardcoding:

- `triple -> (runner, use_cross)` is knowledge about GitHub's runner fleet. Irreducible, but it should be ~4 entries keyed on `target_os`, not 18 keyed on whole triples, and it should be config-overridable for self-hosted runners.
- `target -> "callisto-*.tar.gz"` is product identity compiled into the tool. Not defensible.

**Do not parse target triples by substring.** They are not a stable grammar (`wasm32-wasip1`, `thumbv7neon-unknown-linux-gnueabihf`, `x86_64-fortanix-unknown-sgx`). Ask the toolchain. Verified on stable, with the target NOT installed:

    $ rustc --print cfg --target wasm32-wasip1
    target_arch="wasm32" target_env="p1" target_family="wasm" target_os="wasi"
    $ rustc --print cfg --target x86_64-unknown-linux-musl
    target_arch="x86_64" target_env="musl" target_family="unix" target_os="linux"
    $ rustc -vV | grep host:     -> host: aarch64-apple-darwin

So: `rustc --print target-list` validates (no allowlist), `--print cfg --target` gives facts,
and `use_cross` is computed as `target_arch != host_arch || target_env != host_env` rather than looked up. A derived rule works for target 19; the table does not.

**But `rustc` is the cargo implementation, not the design.** The ecosystem-neutral currency is `TargetFacts { os, arch, env }`; each ecosystem supplies enumeration, description and a build strategy. `use_cross` is itself cargo-specific -- Go cross-compiles natively with `CGO_ENABLED=0`, so "needs cross" is not a universal concept.

Placement, per the layering constraints: `Ecosystem` in `callisto-model` is pure data-only functions and must stay so, therefore a resolver that shells out belongs in a Layer 2 crate beside `trait Manifest`, not on the enum.

**Scope check:** `Ecosystem::CANONICAL` is `[Cargo, Npm, Pypi]` and `is_implemented()` matches only those three (`ecosystem.rs:29,84`). Go, Maven, NuGet, Deno and Jsr are in the enum but "demand-gated ... no canonical-identity manifest wired up yet" -- there is no Go package discovery, so a Go target resolver would have nothing to resolve targets for.

**Consequence worth the pre-1.0 slot:** the release workflow's `build-artifact` matrix is hand-copied into five places and kept honest by `table_agreement`. Generated from config plus a resolver, adding a target becomes one line in `callisto.toml`, and the drift check exists only because the copies do.

## 6. Local toolchain hazard

`.prototools` floats `moon = "2"`. moon self-upgraded 2.4.5 -> 2.5.5 mid-session; 2.5.5's Rust plugin requires proto >= 0.60.0-alpha.0, and a stale local proto (0.57.4) then failed `just fmt-check`, blocking every commit via the pre-commit hook. Fixed by `proto upgrade` to **0.62.3 stable** -- no pre-release needed, contrary to the first reading of the error.

CI was unaffected (PR #105 green, 12/12) because `moonrepo/setup-toolchain` installs a current proto per run. The floating major on the build tool is still the odd one out in a repo that pins actions by SHA, `just@1.58.0` and `cross 0.2.5`.
