# Agent guide

Rules for AI agents working in this repo.

## Crates

| Crate | License | Depends on |
| --- | --- | --- |
| callisto-model | MIT | none (external: `michi-graph`, MIT) |
| callisto-manifests | FSL-1.1-MIT | model |
| callisto-graph | FSL-1.1-MIT | model, manifests |
| callisto-cli | FSL-1.1-MIT | model, manifests, graph |
| callisto-fixtures | FSL-1.1-MIT | model (dev-only) |

- `callisto-model` (MIT) must never depend on an FSL crate. It depends on `michi-graph` (MIT, no dependencies of its own) for dependency-graph validation and ordering. `just license-boundary` fails if anything it builds with is FSL.
- Check with `grep -H "^license" crates/*/Cargo.toml` after adding or moving a crate.

## Commands

- Full pipeline: `just ci` (includes the 90% coverage gate). `just test`, `just lint`, `just fmt`.
- Before every push or PR: `just pre-push && just lint && just doc-check && just test`, and `just ci` before marking a PR ready. Judge each by its exit code. Never tail or grep the output for success (a compile error prints no "failed" line), and never report a check as passed that was not run.
- Commits must be signed: the ruleset requires it. Never pass `-c commit.gpgsign=false`. After pushing, `gh api repos/orin-dx/callisto/commits/<sha> --jq .commit.verification.reason` must say `valid`.
- Performance: `just bench` times `status`, `version --dry-run` and `release --dry-run` over generated workspaces and counts the git processes each starts (`CALLISTO_BENCH_SHAPES=1` lists which). A change that adds a git process per package fails review.
- Scoped iteration: one `cargo test -p <crate>` or `cargo clippy -p <crate> --all-targets -- -D warnings` at a time.
  - Not `moon run <project>:test`: its per-project fan-out serializes on `target/`'s build lock.
  - Never run cargo builds in parallel.
- Never `git checkout -- Cargo.lock` as cleanup. A lockfile diff means investigate, not discard.

## Changesets

- Write one with `callisto add --package cargo/callisto-cli:<bump> --summary "..."` (non-interactive). Target `callisto-cli`: its changelog is the one users read. One changeset per user-visible topic. A fix to something not yet released has no changeset.
- Changesets become the project's changelog, so write them for the people who use it, not for the implementer.
- Open with a bold lead-in naming the change as the user sees it, then one line on its value: what they can now do or what changed for them.
- When there is more to say, add short bullets in this order: why it matters, how to adopt it (commands, configuration, migration), and limits they will hit. Use sub-bullets for detail.
- Group a longer entry under bold labels such as **Upgrading** or **Scripting**. Do not use `#` headings; the changelog supplies those.
- Put a breaking change in its own paragraph starting `**Breaking:**` (old behavior, new behavior, what to do), and a step users must take in one starting `**Action:**`. Never inside a bullet: the release PR lists them under "Read before merging" only as paragraphs.
- One idea per bullet. Leave out internals unless the user must know them.

## Invariants

1. Safe Rust only: `unsafe_code = "forbid"`.
2. Manifest edits preserve format: `toml_edit` for TOML; JSON changes that only replace string values are written into the original text by span (`json_text`), anything else keeps key order and fingerprinted indentation. No regex or line edits.
3. File writes go through `callisto_model::atomic::atomic_write`.
4. User-facing errors derive `miette::Diagnostic` with a code and a fix.
5. No emojis in docs or code.
6. No cargo features in shipped crates; behaviour is chosen at runtime (#143). The dev-only `callisto-model/test-util` feature is the exception.
7. No `unwrap()` outside tests (`just lint` denies it). An `expect` names the invariant that makes it unreachable.

## Enforcing an invariant

Use the first rung that works:

1. A type, or a private constructor or field, so the wrong thing cannot compile.
2. An exhaustive `match` with no `_` arm on our own enums.
3. A clippy or rustc lint already in the workspace lints.
4. One small test that drives the real binary or function and checks observable behavior.
5. Nothing.

- No tests that scan source text, or grep docs or workflows for strings that code can expose directly.
- No hand-maintained lists that mirror code (error codes, command names, env names): derive them from the type.
- Prefer a crate already in `Cargo.lock` over a hand-written grammar: `semver` for versions, `miette` and `thiserror` for errors, `clap` derive for args, `serde` for formats, `insta` for snapshots.
- Every new error code, JSON key or flag is permanent after 1.0: prefer fewer, broader ones.
- Before adding a check, delete one that a type now covers.

## Fixing bugs

- Grep for structurally identical code (same algorithm, other file, ecosystem or mode). A sibling with the same defect is in scope now.
- N copies of one logic: unify to one implementation rather than patching each.
- A deliberately deferred sibling gets an explicit record: changeset note, tracked follow-up or code comment.
- A new shared helper migrates every call site in the same change, or the rest are tracked.

## Plugins

Start work through these commands so their checks run:

- Options before deciding: `/guide:explore`. Does it work: `/guide:spike`.
- Bug or review findings: `/guide:fix`. Real feature or shared change: `/guide:build`.
- Verify or find bugs: `/guide:check`. Commit: `/guide:ship`.

Changesets are written with `callisto add` and follow the changeset rules in this file. `/guide:build` gates shared work through a spec; the owner-confirmation rule below still applies.

Pull requests are opened by Entire from the branch's trail: push the branch and create or link its trail (`entire trail create`). Do not open the PR with `gh`.

## Specs and plans

- Specs (`spec@1`) live in `docs/specs/`, one per area, current behavior only. Change the spec in the same PR as the behavior. No history, rationale or revision notes.
- This repo's release workflow has no GitHub Environment or reviewer gate and only `execute` gets registry secrets (owner policy, enforced by `.github/tests/verify-release-workflow-policy.sh`). Never add one.
- Read `ARCHITECTURE.md` and the matching `docs/architecture/` topic before changing behavior in an area.
- Read `docs/adr/README.md` before changing release flow, Git access, publishing, distribution, platform packages or manifest writes. Reversing an ADR needs a new ADR the owner accepts.
- Plans (`plan@1`) live in `docs/projects/`. Delete a plan once its work ships.
- Code never cites specs: no spec IDs, criterion IDs, section numbers or track names in comments, test names or messages. Describe the behavior instead.
- A requirement the owner didn't state is a judgment call. Get owner confirmation before it gates anything.

## Writing

- Succinct: short sentences, bullets, tables. No backstory, hedging or filler.
- Never hard-wrap Markdown or PR bodies: one line per paragraph.
- Code comments: one short line, only for a non-obvious why. Wrap at 120 columns (`rustfmt.toml`). No history ("used to", "previously"): git records it.
- A commit or PR may end with the `Claude-Session:` trailer. Keep claude.ai session links out of source and documentation files.

<!-- entire-agent:begin -->
Read .entire/agent-guide.md for this repository's workflow, source inspection, and verification guidance.
@.entire/agent-guide.md
<!-- entire-agent:end -->
