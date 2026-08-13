# SPEC-APPLY-BATCH-002 post-T07 verification notes

Recorded evidence for AC-008, AC-009, AC-013, AC-017 (see .claude/specs/SPEC-APPLY-BATCH-002.json), confirmed after T07's batching restructuring landed.

## AC-009 / AC-008: full regression suite

`cargo test -p callisto-graph --lib -- apply::tests::apply_persists_bumps_loop_write_version_to_disk apply::tests::apply_persists_rewrites_loop_update_dependency_spec_to_disk apply::tests::rewrites_loop_update_dependency_spec_error_leaves_manifest_untouched_and_skips_persist apply::tests::apply_version_plan_cargo_bump_produces_byte_identical_output_to_direct_mutate_then_persist apply::tests::apply_version_plan_npm_bump_produces_byte_identical_output_to_direct_mutate_then_persist apply::tests::bumps_loop_write_version_error_leaves_manifest_untouched_and_skips_persist apply::tests::bumps_loop_persist_failure_leaves_earlier_successful_write_intact_and_later_manifest_unchanged apply::tests::rewrites_loop_persist_failure_leaves_earlier_successful_write_intact_and_later_manifest_unchanged apply::tests::cargo_only_bump_does_not_stage_python_lockfile apply::tests::refresh_lockfiles_calls_cargo_update_workspace_when_cargo_bumped apply::tests::refresh_lockfiles_false_does_not_call_cargo_update apply::tests::apply_is_idempotent_when_manifest_already_at_target_version apply::tests::apply_returns_error_when_manifest_has_unexpected_version apply::tests::apply_stages_changeset_path_even_when_file_already_deleted` -- observed result: `test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 198 filtered out`. All 14 pre-existing tests named by AC-009 are individually confirmed present and passing, unmodified:

- apply_persists_bumps_loop_write_version_to_disk
- apply_persists_rewrites_loop_update_dependency_spec_to_disk
- rewrites_loop_update_dependency_spec_error_leaves_manifest_untouched_and_skips_persist
- apply_version_plan_cargo_bump_produces_byte_identical_output_to_direct_mutate_then_persist
- apply_version_plan_npm_bump_produces_byte_identical_output_to_direct_mutate_then_persist
- bumps_loop_write_version_error_leaves_manifest_untouched_and_skips_persist
- bumps_loop_persist_failure_leaves_earlier_successful_write_intact_and_later_manifest_unchanged
- rewrites_loop_persist_failure_leaves_earlier_successful_write_intact_and_later_manifest_unchanged
- cargo_only_bump_does_not_stage_python_lockfile
- refresh_lockfiles_calls_cargo_update_workspace_when_cargo_bumped
- refresh_lockfiles_false_does_not_call_cargo_update
- apply_is_idempotent_when_manifest_already_at_target_version
- apply_returns_error_when_manifest_has_unexpected_version
- apply_stages_changeset_path_even_when_file_already_deleted

AC-008 cites the first two of these directly.

## AC-013: WorkspaceCargoResolver source-diff unmodified

T07 commit: c736bbcfc8250c85f94c67919319c1814c5064db

Located via `git log --format=%H --grep='batch same-path Manifest-trait writes into one open/mutate/persist cycle' -1`, confirmed unique (a single matching commit; subject `fix(graph): batch same-path Manifest-trait writes into one open/mutate/persist cycle`, cross-checked with `git log --oneline | grep -i "batch same-path"`).

`git diff --name-only c736bbcfc8250c85f94c67919319c1814c5064db^ c736bbcfc8250c85f94c67919319c1814c5064db` shows T07's commit touched exactly two files: `crates/callisto-graph/src/apply.rs` and `crates/callisto-graph/tests/apply_persist_open_count_test.rs`. `crates/callisto-manifests/src/cargo.rs` does not appear in T07's diff at all, so `WorkspaceCargoResolver::write_version` and `WorkspaceCargoResolver::write_dependency` are unmodified by this commit.

`git diff c736bbcfc8250c85f94c67919319c1814c5064db^ c736bbcfc8250c85f94c67919319c1814c5064db -- crates/callisto-graph/src/apply.rs | grep -E '^-' | grep -E 'ws_res\.(write_version|write_dependency)'` produced no output -- no removed or changed line touches a `ws_res.write_version`/`ws_res.write_dependency` call site. Direct inspection of the diff hunks confirms zero lines referencing `VersionWriteTarget::CargoWorkspacePackage` or `DepWriteTarget::CargoWorkspaceDependency` appear in the diff at all (the match arms at their current locations in apply.rs, lines 204 and 232, are untouched by this commit) -- the diff's 52 added / 3 removed lines in apply.rs are confined to the new `classify_manifest_writes`-driven batching logic and the `if !classification.excluded.contains(p) { continue; }` guard around the surrounding `Manifest(p)` arms, not the CargoWorkspacePackage/CargoWorkspaceDependency arms themselves.

## AC-017: bump-before-rewrites ordering is not byte-observable, correcting T13's commit-message evidence

T13's commit (`1234bef`) recorded a blanket claim that `CargoToml::update_dependency_spec` "only touches a dependency-table entry with a plan-time-constant value, no read of `current_version()`/other state." An independent review of that commit caught that this claim is incomplete for one branch, and since commit messages cannot be corrected after the fact, the complete evidence is recorded here instead.

Read `crates/callisto-manifests/src/cargo.rs` directly:

- `CargoToml::write_version` (lines 200-242): touches only `[package].version` in the direct-write branch, or does an explicit pinned replacement of that same field for a workspace-inherited member. No other state is read or written. Claim holds without qualification.
- `CargoToml::update_dependency_spec` (lines 290-377): for a **non-inherited** dependency (lines 320-376, the case both of T13's AC-014/AC-016 fixtures and this file's own AC-005/AC-006 fixture exercise), the claim holds exactly as stated -- a disjoint dependency-table entry is mutated with a plan-time-constant `DepSpec`, no read of `current_version()`. For an **inherited** dependency (lines 305-318), the method instead delegates to `WorkspaceCargoResolver::load(&root_cargo)` followed by `ws_res.write_dependency(name, new, permit)` -- an immediate, synchronous read-and-persist against a **different physical file** (the workspace root `Cargo.toml`), not the manifest whose handle `update_dependency_spec` was called on. `WorkspaceCargoResolver::write_dependency` calls `self.persist(permit)` internally (cargo.rs:709), and `WorkspaceCargoResolver::persist` (cargo.rs:715-723) does not call `crate::record_persist_call()`, so this delegated write is invisible to `persist_call_count()`.

This does not weaken AC-017's actual claim: the delegated write lands on a file distinct from the one being batched in the group under discussion, so it has no bearing on whether bump-before-rewrites ordering is byte-observable *within that group's own manifest*. It also does not contradict this spec's own scope: `non_goals` already states that `WorkspaceCargoResolver`-routed writes remain immediate, per-entry, and outside `persist_call_count`'s instrumentation (which is scoped exclusively to the `Manifest` trait's three implementors). The correction here is one of completeness in T13's evidence write-up, not a functional defect in the shipped code -- verified via `cargo test -p callisto-graph --lib apply::tests` (22 passed) and `cargo clippy -p callisto-graph --all-targets -- -D warnings` (clean) after this note was added, confirming no code change was needed or made.
