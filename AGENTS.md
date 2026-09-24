# Agent guide

Rules for AI agents working in this repo.

## Crates

| Crate | License | Depends on |
| --- | --- | --- |
| callisto-model | MIT | none |
| callisto-format | MIT | model |
| callisto-vcs | MIT | model |
| callisto-manifests | FSL-1.1-MIT | model |
| callisto-conventional | FSL-1.1-MIT | model |
| callisto-changelog | FSL-1.1-MIT | model |
| callisto-graph | FSL-1.1-MIT | model, vcs, manifests, format, changelog, conventional |
| callisto-cli | FSL-1.1-MIT | all of the above |
| callisto-fixtures | FSL-1.1-MIT | model (dev-only) |

- MIT crates must never depend on FSL crates.
- Check with `grep -H "^license" crates/*/Cargo.toml` after adding or moving a crate.

## Commands

- Full pipeline: `just ci` (includes the 90% coverage gate). `just test`, `just lint`, `just fmt`.
- Scoped iteration: one `cargo test -p <crate>` or `cargo clippy -p <crate> --all-targets --all-features -- -D warnings` at a time.
  - Not `moon run <project>:test`: its per-project fan-out serializes on `target/`'s build lock.
  - Never run cargo builds in parallel.
- Changesets: `callisto add --package <crate>:<bump> --summary "..."` (non-interactive).
- Never `git checkout -- Cargo.lock` as cleanup. A lockfile diff means investigate, not discard.

## Invariants

1. Safe Rust only: `unsafe_code = "forbid"`.
2. Manifest edits preserve format: `toml_edit` for TOML; JSON keeps key order and fingerprinted indentation. No regex or line edits.
3. File writes go through `callisto_model::atomic::atomic_write`.
4. User-facing errors derive `miette::Diagnostic` with a code and a fix.
5. No emojis in docs or code.

## Fixing bugs

- Grep for structurally identical code (same algorithm, other file, ecosystem or mode). A sibling with the same defect is in scope now.
- N copies of one logic: unify to one implementation rather than patching each.
- A deliberately deferred sibling gets an explicit record: changeset note, tracked follow-up or code comment.
- A new shared helper migrates every call site in the same change, or the rest are tracked.

## Specs and plans

- Specs (`spec@1`) live in `docs/specs/`; plans (`plan@1`) in `docs/projects/`.
- Delete a plan once its work ships. Keep a spec while it describes current behavior.
- A requirement the owner didn't state is a judgment call. Get owner confirmation before it gates anything.

## Writing

- Succinct: short sentences, bullets, tables. No backstory, hedging or filler.
- Never hard-wrap Markdown or PR bodies: one line per paragraph.
- Code comments: one short line, only for a non-obvious why.
- No claude.ai session links in commits, PRs or files.
