# Session Handoff — 2026-08-12

Context for picking up in a fresh session. This session ran very long (Track F, Track G, a full
workspace audit, and an 8-bug remediation pass) and built up a lot of context — read this file
instead of trying to reconstruct history from scratch.

---

## 1. What shipped this session

**Tracks B, E, F, G** — all implemented via the canon→vector→lambda SDD pipeline (spec → adversarial
spec audit → plan → adversarial plan challenge → TDD implementation → review per commit → final
adversarial exit-gate), all passing `just ci`:
- Track B: idempotent `apply_version_plan` + lockfile refresh
- Track E: `PackageId` specificity + cross-ecosystem diagnostic
- Track F: `resolve_package_config` extraction (two-pass `[[package]]` specificity)
- Track G: `callisto matrix` — napi-rs + maturin native build target auto-discovery, npm/PyPI
  runtime-version constraints. New CLI subcommand, new model types, wired into `callisto schema`
  and the `nativeMatrix` CI output.

Also landed: ~44 files (13 feature units) that were found already-written, uncommitted, in the
working tree at session start (author/origin unknown) — committed in logical batches. **This code
did NOT go through canon→vector→lambda** — see §3, this is exactly where the regressions came from.

**Full workspace audit** — 24-agent adversarial workflow (8 parallel dimension-finders: architecture,
domain modeling, correctness, security, performance, dependencies, tests, docs → adversarial
verification of critical/high findings → synthesis). Report published as an Artifact:
https://claude.ai/code/artifact/80f347f5-afea-488c-886b-518bea99a458
Raw source (session-scratch path, may not survive): `audit-report.md` was written to this session's
scratchpad — **do not rely on it existing**; the artifact URL and the memory file below are the
durable copies.

**8-bug remediation** — every CONFIRMED critical/high finding from the audit fixed via strict TDD
(red test reproducing the exact reported failure scenario → confirmed failing → minimal fix →
confirmed passing → independent read-only review → commit). One review-found gap (a fix that had
correct logic but an untested branch) was closed rather than accepted. See §2 for the list.

## 2. The 8 bugs fixed this session (all committed, all reviewed, all on `main`)

1. `bca04b1` — `cargo_publish`'s pre-publish version guard couldn't parse `version.workspace = true`
   (the pattern all 10 of this repo's own crates use). Would have hard-failed `callisto publish` on
   this repo today. **This bug had ZERO test coverage when it shipped — see §3.**
2. `a700bba` — `PublishTarget::NuGet`/`GitHubRelease` silently no-opped the real publish while the
   release-tag gate still fired, claiming success with nothing published. Now an exhaustive match +
   ecosystem-mismatch validation at walk time (E119).
3. `2bb6ca4` + `c09c5f7` — arbitrary file write via unvalidated `changelog` path in `callisto.toml`,
   plus `changesets.dir`'s traversal guard missing the absolute-path case. Both now route through
   `callisto_model::path::workspace_relative`.
4. `bb194cc` — npm's manifest-controlled `publishConfig.registry` field was unvalidated and could
   redirect `npm publish`/`npm view` to an attacker host with `NPM_TOKEN` live in the environment.
   Now validated against the operator's `[registries]` allowlist (E120).
5. `6e16212` + `6b09e48` — subprocess timeout only killed the direct child, not descendants holding
   stdout/stderr pipes open, so `PUBLISH_TIMEOUT_SECS` could be defeated forever. Fixed with a
   bounded reader-thread grace period (channel-based, no `unsafe`, no process-group/libc calls —
   `unsafe_code = "forbid"` is a hard workspace invariant). Both the timeout-kill branch and the
   normal-exit branch are now independently tested.
6. `d353e54` — Cargo `round_trip` over-corrected a prior bound-crossing fix, bailing on ALL compound
   ranges with an upper bound instead of just ones the target actually crosses. The regression test
   that shipped alongside the original bug was named `..._is_rewritten` but asserted `is_none()` —
   the test documented the bug instead of catching it.
7. `def6862` — `[[package-set]]` ecosystem-prefixed glob patterns (`"cargo:pkg-*"`) matched zero
   packages, silently, because matching only ever checked the bare name. Independently rediscovered
   by 3 of the 8 audit dimension-finders from different angles. `GraphError::PackageSetMatchedNothing`
   existed as a safety net but was never constructed anywhere — now a real `DiagnosticCode` fires.

Final `just ci` after all 8 fixes: **confirmed green** (all 7 phases — fmt-check, lint, test, audit,
doc-check, wasm-check, coverage — exit 0, verified clean, not just exit-code-trusted) on `def6862`,
run 2026-08-12. If you're picking this up later and other commits have landed since `def6862`,
re-run `just ci` rather than trusting this note.

## 3. Root cause of the regressions, and the process fix

The 8 bugs above were NOT random — every one of them is in the batch of pre-existing code found in
the working tree at session start (`e0cc091` "publish pipeline hardening", `60e0bef` "package-set
pattern resolution"), never Track F/G's own freshly-built logic. The audit independently confirms
this: Track F/G's code came through the sweep comparatively clean.

**Why:** that batch was committed after checking only "does it compile, does clippy pass, do the
*existing* tests pass" — never run through canon→vector→lambda, never given a fresh adversarial
pass, never TDD'd. One of the bugs (`bca04b1`'s version-check) shipped with **zero test coverage at
all**, not even a happy-path test.

**This is now a saved lesson** — read it before treating any future "found in the working tree"
code as done: `~/.claude/projects/-Users-gabe-Projects-callisto/memory/feedback_found_code_needs_same_rigor.md`
(indexed in `MEMORY.md`). Short version: code you didn't personally build via this session's TDD —
found in a dirty tree, from another agent, copied in — gets the same rigor as freshly-written code,
regardless of whether it compiles and its own tests pass.

**A second, narrower process note:** during Fix 1's review, a lambda-reviewer subagent ran
`git stash`/`git checkout` on its own (reviewers should be strictly read-only) and collided with an
unrelated pre-existing stash entry from another concurrent session (`6b75fea`, still sitting in
`git stash list` — not mine, never touched, leave it alone unless you know what it is). No data was
lost (verified via `git fsck`, full `git reflog show HEAD`, and a full unfiltered
`cargo test --workspace` — 679 passed, 0 failed). Every reviewer prompt from Fix 2 onward explicitly
stated "STRICTLY READ-ONLY — do not run git stash/checkout/reset/clean" and none repeated the issue.
Keep doing that explicitly in every reviewer prompt going forward.

## 4. What's still open — the recommended next step

Four test-coverage gaps the audit named explicitly, NOT yet touched by the remediation pass. These
are concrete, already-scoped, and cheap — either wire the code up with real tests or delete it if
it's genuinely dead:

1. **`npm::round_trip`** (`crates/callisto-manifests/src/npm.rs:395-411`) — zero test coverage
   anywhere; its only reachable path (`preserve_npm_ranges = true`) is never set `true` in any test.
   Also rejects npm's idiomatic space-separated compound ranges (`>=1.0.0 <2.0.0`) with no test
   documenting this as intentional.
2. **`parse_publish_target`/`parse_release_trigger`** (`crates/callisto-graph/src/config/resolve.rs:148-163`)
   — only `"none"`/`"crates-io"` are ever parsed by any test; `"npm"`, `"pypi"`, `"nuget"`,
   `"github-release"`, and the invalid-string error path are untested despite being from this
   session. (Note: fix #2 above already added ecosystem-mismatch validation around this — re-verify
   the exact current shape before writing tests.)
3. **`probe_git`/`check_git_version`** (`crates/callisto-graph/src/locate/git.rs:1-13`) — built to
   give a friendly `IncompatibleVersion` diagnostic for git < 2.20, but never called anywhere outside
   its own definition and has no test. Wire it into `load_workspace` before any git-dependent
   operation, or remove it if it's not wanted.
4. **`pre_cursor` tracking** (`crates/callisto-conventional/src/pre_cursor.rs:1-57`) — zero test
   coverage, and the field it would populate (`TagIndex.pre_cursor`) is hardcoded empty everywhere
   downstream. Looks like a half-wired, abandoned feature — wire it up with tests or remove the dead
   exports; don't leave it half-built.

Beyond these four: the audit's full report has **41 unverified medium/low findings** not covered by
this session's remediation at all (only the 8 critical/high findings that survived adversarial
verification got fixed). Treat these as a prioritized backlog, not a to-do-tonight list — see the
artifact for the full list with file:line and recommendations, grouped by theme (architecture,
modeling, security, performance, dependencies, tests, docs).

**Important calibration, stated directly to the user this session and still true:** fixing these 8
bugs does NOT mean "the entirety of happy path and corner cases" is now covered. The audit itself
was a bounded sampling pass (each finder was told to prefer breadth over exhaustive depth), not a
proof of completeness. There could be more bugs of the exact same class in `e0cc091`/`60e0bef` that
weren't found.

## 5. Where things live

- Memory index: `~/.claude/projects/-Users-gabe-Projects-callisto/memory/MEMORY.md` — read this at
  the start of any new session on this repo.
- Active-work ledger: `.claude/plans/ACTIVE.md` — update it once this handoff's next step is decided.
- Specs (SPEC-001 through SPEC-004): `.claude/specs/`
- Plans: `.claude/plans/track-*-plan.json`
- Semantic model (progressive-load reference docs): `.claude/semantic-model/INDEX.md`
- Audit report (durable): the Artifact URL in §1, plus the memory file
  `project_workspace_audit_2026-08.md`

## 6. Starter prompt for the fresh session

```
Read .claude/plans/SESSION_HANDOFF.md and the memory index at
~/.claude/projects/-Users-gabe-Projects-callisto/memory/MEMORY.md before doing anything else.

`just ci` was confirmed green on `def6862` when this handoff was written (2026-08-12) — if `main`
has moved since, re-run `just ci` first rather than trusting that. Then close the four named
test-coverage gaps in SESSION_HANDOFF.md §4
(npm::round_trip, parse_publish_target/release_trigger, probe_git wiring, pre_cursor) one at a time
via strict TDD — red test first, confirmed failing, minimal fix or deletion, confirmed passing,
independent read-only review per fix (explicitly tell every reviewer agent it is read-only and must
not run git stash/checkout/reset/clean), commit. Don't batch them.

After that, tell me what you'd recommend for the remaining 41 unverified medium/low audit findings
(full list at the artifact URL in the handoff doc) rather than starting on them without asking —
that's a scope/priority call I want to make, not one to assume.
```
