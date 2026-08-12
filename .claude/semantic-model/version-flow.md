# Version Flow — Semantic Model

Sources: `crates/callisto-graph/src/apply.rs`, `crates/callisto-graph/src/plan.rs`

## VersionPlan

The complete description of what needs to change. Produced by plan-generation, consumed by apply.

Key fields:
- `bumps: Vec<PlannedBump>` — version changes per package
- `rewrites: Vec<PlannedRewrite>` — dependency spec updates in manifests
- `changelog_writes: Vec<ChangelogWrite>` — changelog sections to prepend
- `consumed_changesets: Vec<PathBuf>` — changeset files to delete after apply
- `pre_state_update: Option<PreState>` — pre-mode state to write to pre.json
- `delete_pre_json: Option<PathBuf>` — pre.json to delete (exit pre-mode)

`VersionPlan` derives `Serialize`/`Deserialize`/`JsonSchema` but is not currently persisted.
This is a known gap — persisting it would enable Release Please-style resumable applies.

## PlannedBump

```rust
pub struct PlannedBump {
    pub package: PackageId,
    pub from: Version,        // expected current version before apply
    pub to: Version,          // target version after apply
    pub severity: Severity,
    pub governed_by: Option<GroupName>,
    pub reason: Option<String>,
    pub writes: Vec<VersionWriteTarget>,
}
```

`from` and `to` are the idempotency anchor: the manifest's current on-disk version must equal
either `from` (proceed with write) or `to` (skip write, already applied).

## VersionWriteTarget

```rust
pub enum VersionWriteTarget {
    Manifest(PathBuf),                          // regular package manifest
    CargoWorkspacePackage { root_manifest: PathBuf }, // [workspace.package] version
}
```

For `CargoWorkspacePackage`, version is read via `WorkspaceCargoResolver::workspace_version()`.
For `Manifest`, version is read by opening the manifest handle and reading the version field.

## ApplyOutcome

```rust
pub struct ApplyOutcome {
    pub lockfile_refresh_results: Option<Vec<LockfileRefreshResult>>, // always None currently
    pub staged: Vec<PathBuf>, // paths passed to `git add` or `git rm --cached`
}
```

`staged` is the single source of truth for what was touched. It includes:
- All manifest paths (written or skipped-as-idempotent)
- All changeset paths (deleted or not — staged unconditionally)
- All changelog paths
- Lockfile paths (Cargo.lock, package-lock.json, etc.) for active ecosystems

## apply_version_plan — Idempotency Guard

### The Contract (Track B design decision)

For each `VersionWriteTarget::Manifest(p)` in a bump's writes:

1. Open the manifest and read the current version
2. If current == bump.from → call write_version, push p to modified_paths
3. If current == bump.to → skip write_version, still push p to modified_paths (idempotent retry)
4. If current is anything else → return Err(GraphError::UnexpectedManifestVersion { path: p, expected_from: bump.from, expected_to: bump.to, found: current })

For `VersionWriteTarget::CargoWorkspacePackage { root_manifest }`:
Same logic, but read current version via `WorkspaceCargoResolver::workspace_version()`.

### Changeset Staging

Changeset paths in `plan.consumed_changesets` are ALWAYS pushed to `modified_paths`,
regardless of whether the file exists on disk. This enables `git rm --cached --ignore-unmatch`
to clean the index on idempotent retry after a crash deleted the file but didn't stage the removal.

The current code has a bug here: it gates the push behind `if full.exists()`. Track B removes
this gate.

### Why manifest path is staged even when write is skipped

On an idempotent retry, the manifest was already written in the prior (crashed) run. The file
on disk is correct but may not be staged in git. Pushing it to modified_paths ensures
`git add` re-stages it, making the index consistent even when no bytes were changed.

## Infrastructure Already in Place

- `GraphError::UnexpectedManifestVersion` — error.rs, E117, fields: path, expected_from, expected_to, found
- `WorkspaceCargoResolver::workspace_version()` — cargo.rs, returns `Result<Option<Version>, ManifestError>`
- Three RED tests in apply.rs test module — see `.claude/plans/ACTIVE.md` for names

See `.claude/specs/track-b-idempotent-apply.json` for the full testable acceptance criteria.
