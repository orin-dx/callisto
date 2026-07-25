# Research brief: library-first vs moon-first, and the polyglot versioning landscape

**Purpose:** This is a handoff document for a deeper investigation than a chat session can
do well — reading actual source code, tracing actual architecture decisions, and forming a
confident recommendation on the one open question blocking `01-spec.md`: **should
callisto's coordination core be designed as a standalone library with moon as one
integration point, or moon-first with the library question deferred?**

Companion document: `00-design.md` (the current spec). Read that first — this brief assumes
its content and doesn't repeat context already established there. Section references below
(§0.1, §15, etc.) refer to that document.

---

## 1. The core question, precisely stated

`00-design.md` §0.1 identifies three options:

- **Option A (moon-first):** `callisto-graph` and friends are designed around moon's
  project graph as a first-class input. Standalone CLI usage degrades gracefully but isn't
  the design center. Simpler to build initially; couples the core to moon's API surface
  and its stability guarantees.
- **Option B (library-first):** Core crates have zero moon dependency, exposing a clean API
  that moon, a GitHub Action, a future Nx plugin, or a bare pre-commit hook can all consume
  identically, with none privileged. More upfront design discipline; likely more
  future-proof; possibly more work now for a payoff that may not materialize if moon
  integration is the only integration anyone ever wants.
- **Option C (hybrid):** Library-first internally, but moon is the reference integration we
  actually ship, document, and support first — because that's our real user base today.
  Adding a second integration later should be cheap, without us needing to build that
  second integration now.

The current draft of `00-design.md` leans Option C without having rigorously earned that
lean — it's closer to "sounds reasonable" than "we traced how the analogous decision played
out for comparable tools and it clearly worked/didn't." That's the gap this research should
close.

### What "library-first" would concretely cost or buy

Things worth quantifying, not just asserting:

- **Cost:** does zero-moon-dependency in `callisto-graph` force worse design elsewhere?
  E.g., if moon's project graph is genuinely the best source of cross-ecosystem dependency
  truth (it already resolves Cargo + npm + TS project references + explicit `dependsOn`),
  does building a from-scratch `ManifestWalkResolver` mean duplicating real engineering that
  moon has already solved well? Is the duplication worth it for the optionality, or is it
  wasted effort chasing a hypothetical second integration that may never come?
- **Buy:** if we do it library-first, what does that concretely unlock? A Nx plugin? A
  Bazel rule? A pure pre-commit hook with no build-system integration at all? Are any of
  those genuinely plausible near-term, or is "library-first" solving for an audience that
  doesn't exist yet?
- **Precedent cost/benefit:** how did release-please's library-first design actually play
  out? Did the CLI and the Action genuinely benefit from sharing one core, or did the
  library abstraction introduce friction (versioning the library independently of the CLI,
  keeping the API stable across releases, etc.)?

---

## 2. What's already been established from chat-based research (starting point, not final)

This was gathered via web search and fetch in the prior conversation — real but shallow.
Verify, deepen, and correct as needed; don't take it as settled.

### `release-please` (googleapis)

- Ships as an npm library (`release-please` package) exporting a `Manifest` class and the
  releaser/plugin machinery directly. The CLI and the `release-please-action` GitHub Action
  are both built as consumers of this library — this is the closest existing precedent for
  Option B/C.
- Two-file config model: `release-please-config.json` (declarative — release types, plugins,
  per-package overrides) + `.release-please-manifest.json` (state — current version per
  package, committed to git, read/written by every run).
- Plugin architecture: a `ManifestPlugin` base class with a `run(pullRequests)` method that
  receives the array of candidate release PRs *after* individual per-package releasers have
  run, and can merge/mutate them before the final PR is created. Plugins operate with full
  cross-package context; individual releasers only see one package.
- Per-ecosystem "workspace" plugins exist as separate plugins, not baked into the core:
  `node-workspace`, `cargo-workspace`, `maven-workspace`. Each builds its own dependency
  graph for its ecosystem and does patch-cascade + changelog + PR-list updates. Notably:
  when the Rust releaser runs *standalone* (not manifest-mode) it tries to update monorepo
  deps itself without building a real crate graph; when run in *manifest* mode alongside the
  `cargo-workspace` plugin, the releaser does NOT do this and defers entirely to the plugin.
  This bifurcation (same releaser, different behavior depending on which mode it's invoked
  in) is worth understanding in more depth — is it a wart, or a deliberate layering?
- `linked-versions` plugin groups packages that should share a version when jointly
  releasing (their "linked" ≈ callisto's `linked-group`, roughly).
- `group-priority` plugin: if multiple release "groups" (e.g., snapshot vs. stable) would
  produce PRs simultaneously, restrict to the highest-priority group only — avoids mixing
  snapshot and non-snapshot bumps in the same PR. **Callisto's design doesn't currently have
  an equivalent** — worth evaluating whether our fixed/linked-group model has an analogous
  gap around simultaneous pre-release + stable-release changesets.
- Tag convention: `<component>-v<version>` by default, with an `include-component-in-tag:
  false` override to just `v<version>`. Directly comparable to callisto's `tag_template`.
- Known real limitation: issue #2207 documents exactly callisto's target case failing —
  "Node project wrapping a Rust workspace," where the user wants every component (regardless
  of which file changed) to bump to the same next version. This is worth reading in full;
  it may directly validate or complicate callisto's fixed-group model.
- Config knobs worth comparing against callisto's cascade config: `always-link-local`
  (whether local deps are always bumped even outside declared SemVer range — this is
  release-please's version of callisto's `Always` cascade mode), `bump-minor-pre-major` /
  `bump-patch-for-minor-pre-major` (0.x-specific bump remapping — **this is exactly the
  "smoothing" behavior callisto's §6.2 explicitly rejects**; worth understanding why
  release-please offers it as opt-in rather than never-do-this, since that's a direct
  disagreement worth resolving with eyes open rather than by omission).

### Nx Release

- Actively rewriting its versioning internals (Nx v21, 2025) specifically to "enhance
  flexibility and allow for better cross-ecosystem support" — an automated migration was
  provided for existing configs. This is directly relevant: a mature, well-resourced tool
  concluded its *original* versioning architecture didn't generalize well across ecosystems
  and had to be reworked. Worth understanding exactly what broke and why, in detail — this
  is the closest thing to "someone already ran this experiment and hit a wall."
  Legacy-versioning opt-out existed for one release cycle (v21) and was fully removed in
  v22 — a two-version deprecation window, worth noting as a precedent for how aggressively
  (or not) to force migrations if callisto ever needs an equivalent internal rework.
- Handles npm, Docker, Rust (via community `@monodon/rust` plugin) natively; Java/Gradle and
  .NET are on the announced roadmap as of the 2024/2025 blog posts. Plugins are TypeScript;
  the stated rationale is "keep development familiar and extensible" even as the
  performance-critical core moves to Rust — a different tradeoff than callisto's
  all-Rust-core posture, worth understanding whether that's a meaningful design lesson or
  just a different team's different constraints (Nx's plugin authors are a broad, external
  TS-fluent community; callisto likely won't have that until much later, if ever).
- For Rust specifically: Nx Release prompts once for a version bump and applies it to all
  crates in the release by default ("all crate versions are kept in sync") — this reads as
  closer to a single fixed-group-for-everything default than callisto's more granular
  per-package model. Worth understanding whether that's a simplicity win users actually
  prefer, or a limitation the community plugin hasn't solved yet.

### `semantic-release` and its monorepo extensions

Not yet researched in this thread — flagged as a gap. `semantic-release` itself is
single-package by design; the ecosystem's monorepo answer is a separate tool
(`multi-semantic-release` or similar community extensions). Worth understanding:
- How does `semantic-release`'s plugin architecture work (it's one of the most mature
  plugin systems in this space — `@semantic-release/commit-analyzer`,
  `@semantic-release/release-notes-generator`, `@semantic-release/npm`, etc., composed via
  a config array)? Is there a "library core, plugins are consumers" split analogous to
  release-please's, or is it structured differently (e.g., plugins are the *only* way to
  add capability, with no separate library surface)?
- Why did the monorepo story end up as a bolt-on extension rather than a first-party
  feature? Is that informative for whether callisto's polyglot-monorepo story should be
  core or a plugin?

### `knope`

Already covered in `00-design.md`'s prior-art table but worth a deeper source read if time
allows: specifically how `knope-versioning` (the crate) is structured internally, whether
its own internal architecture separates "format," "versioning," and "workspace resolution"
the way callisto's crate split does, or whether it's more monolithic. This is the closest
prior art in Rust specifically (not just conceptually), so its actual crate boundaries are
worth comparing line-for-line against `00-design.md` §15's proposed `callisto-format` /
`callisto-model` / `callisto-graph` split.

### `cargo-release`

Rust-only, not polyglot, but worth understanding for one specific thing: how it handles the
"which crates in the workspace actually need a release" question (its "change detection to
help guide in what crates might not need a release" feature) — this is conceptually close
to callisto's stateless `last_tag_for` detection primitive and empty-changeset validation
(§6.3). Does `cargo-release` do this via git diff against last tag, via `cargo-semver-checks`
style API comparison, or something else? Any technique here that callisto's empty-changeset
validation is currently missing?

---

## 3. Concrete research tasks

In rough priority order. Each should produce a written finding (a paragraph or two, with
source citations) rather than just a "yes/no" — the goal is to arrive at a well-argued
recommendation on §0.1's Option A/B/C question, not just more raw notes.

1. **Clone and read `release-please`'s actual source**, specifically:
   - The `Manifest` class's public API surface (what does the library export, and what's
     the CLI/Action-specific glue that sits outside it?)
   - The `ManifestPlugin` base class and at least two concrete plugin implementations
     (`node-workspace`, `cargo-workspace`) to see how much of the "cross-package cascade"
     logic lives in the plugin vs. the core.
   - Where the line is drawn between "library" and "CLI-only" — is there anything the CLI
     does that isn't reachable through the library API? If so, that's a data point against
     pure Option B.

2. **Read the Nx v21 versioning rewrite's actual changes** (migration guide, PR history if
   accessible, RFC/discussion if one exists) to understand precisely what broke in the old
   design and what the new design does differently. This is the most valuable single
   artifact for de-risking callisto's own architecture, because it's a real "we designed
   this wrong the first time" data point from a comparable, well-resourced team.

3. **Investigate `semantic-release`'s plugin architecture and its monorepo extensions** per
   §2 above — currently an unresearched gap.

4. **Read `knope-versioning`'s actual crate source** and compare its internal module
   boundaries against callisto's proposed `format`/`model`/`graph` split.

5. **Form a recommendation on §0.1** (Option A / B / C) with an explicit argument, not just
   a preference. The argument should address:
   - Given what release-please and Nx Release actually did, is "library-first" a real
     pattern worth following, or did it emerge for reasons specific to those tools' scale
     and multi-consumer needs (e.g., release-please needing to support hundreds of Google
     repos with different CI setups) that don't apply to callisto's much narrower moon+
     changesets-format niche?
   - Is there a *minimal* version of library-first (e.g., just keeping `callisto-graph`'s
     `DependencyResolver` trait moon-agnostic, per §15's "load-bearing design commitment,
     regardless of how §0.1 resolves") that captures most of the benefit without the cost
     of designing three equally-weighted consumer surfaces up front?
   - What would change in `00-design.md` §15 (crate layout) if the answer is "moon-first,
     don't overthink it" vs. "fully library-first"? Be concrete about which crates merge,
     split, or change their public API shape under each answer.

6. **Investigate whether release-please's `group-priority` plugin (simultaneous
   snapshot/stable release groups) reveals a gap in callisto's fixed/linked-group model.**
   Callisto currently doesn't have an explicit mechanism for "if a snapshot-mode changeset
   and a stable-mode changeset both exist, only act on one." Is this a real gap, or does
   callisto's existing pre-mode (§8) already cover this case adequately by construction?

7. **Investigate release-please's `bump-minor-pre-major` / `bump-patch-for-minor-pre-major`
   config options** against callisto's §6.2 hard "no 0.x remap, ever" stance. Is
   release-please's opt-in flexibility here a sign that real users want the remap behavior
   sometimes, which would argue for callisto offering it as an opt-in rather than refusing
   to support it at all? Or is it scope creep worth avoiding? Form an explicit
   recommendation — this is a case where the current design (§6.2) is more rigid than a
   comparable mature tool, and that rigidity should be either deliberately defended or
   revised, not left unexamined.

8. **Investigate the release-please issue #2207 thread in full** (comments, any linked
   PRs/discussion, maintainer responses) — it's the closest documented real-world case of
   exactly callisto's target scenario (Node-wrapping-Rust, want-everything-to-bump-together)
   failing in a mature polyglot tool. Any maintainer commentary on *why* it's hard, or what
   a real fix would require, is high-value input for callisto's fixed-group design.

---

## 4. Deliverable

A revised `00-design.md` §0.1 and §15 (or a standalone `02-library-vs-moon-decision.md` that
`01-spec.md` can cite), containing:

- A clear recommendation: A, B, or C, with the argument from task 5 above.
- If C (hybrid) or B (library-first): the concrete crate-boundary and trait-signature
  implications for §15 — specifically, does `DependencyResolver`'s trait shape change, does
  a new crate emerge for "integration-agnostic core," does `callisto-cli` change shape.
- If A (moon-first): an explicit acknowledgment of what's being traded away (the
  optionality Option B/C would have preserved) and why that trade is acceptable given
  callisto's actual near-term user base.
- Answers to research tasks 6 and 7 above, folded into `00-design.md` §7.4/§7.5 (cascade
  and groups) and §6.2 (bump_version rigidity) respectively, as either confirmed design
  decisions or revised ones.

This deliverable is the input to `01-spec.md` (full trait signatures, function contracts,
per-crate API surface) — that document should not be started until this one lands, since
the crate boundaries it would pin down are exactly what's unresolved here.
