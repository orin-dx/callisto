---
callisto-cli: minor
---

**Render the `governed by <key> = <value> (default)` attribution line for bumps and diagnostics (§13 invariant 28)**

`render::attribution::attribution_line` has existed since the config-provenance work landed but had zero callers: `render_version`'s text output printed a bump's package/from/to and a diagnostic's severity/message, but never consulted `BumpRecord.governed_by` / `Diagnostic.governed_by` at all, so an operator running `callisto version` (or `--format text` generally) had no way to see which `callisto.toml` key was responsible for a bump or warning, or whether that key was set explicitly or left at its default.

`render_version` now takes the workspace's `&ResolvedConfig` and prints the attribution line under any bump or diagnostic whose `governed_by` is `Some`, via the same `attribution_line` the spec always intended to render it (`callisto-graph` computes the key, `callisto-cli` looks up and renders its current value and provenance -- neither crate re-derives the other's half). `render_diagnostics` gained a matching `Option<&ResolvedConfig>` parameter so it can do the same for any report's diagnostics; every caller other than `render_version` passes `None` today, since no other report ever populates `governed_by` on a `Diagnostic` yet, so their output is unchanged.

**Signature change**: `render::render_version(report, w)` is now `render::render_version(report, cfg, w)`. `render::render_diagnostics(diagnostics, w)` is now `render::render_diagnostics(diagnostics, cfg, w)`. Both are `callisto-cli`'s own rendering functions with one production call site each (`commands/version.rs`); no other crate calls them today.

Not included: `Diagnostic.governed_by` attribution for `StatusReport`, `PublishReport`, `PublishPlan`, `ValidateReport`, and `MatrixReport` is still a no-op, because none of those report kinds ever construct a diagnostic with `governed_by: Some(_)` today -- wiring it up now would mean threading `ResolvedConfig` through several more CLI command handlers for a code path with nothing to render yet. Tracked as a follow-up for whenever one of those diagnostics starts carrying a real `governed_by`, rather than done speculatively here.
