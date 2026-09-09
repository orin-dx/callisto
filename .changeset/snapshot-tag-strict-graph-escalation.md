---
callisto-cli: patch
---

`snapshot` and `tag` now share a new `abort_on_crosscheck_failures` helper (`crates/callisto-cli/src/commands/mod.rs`) instead of each hand-rolling an identical ~15-line block that cloned `ws.graph.diagnostics()`, called `callisto_graph::commands::escalate`, filtered for `Error` severity, and formatted the same abort message.

**Fix**: that duplicated block called `escalate(&mut diags, true, true)` -- hardcoding both the `strict` and `strict_graph` arguments to `true` -- instead of passing through the command's actual flags, the way `status`, `validate`, and `version` already do via their `*Options` structs. In practice this meant `--strict-graph` had no CLI flag at all on `snapshot`/`tag` and no way to take effect on its own: the escalation block only ran from behind an `if args.strict` gate, so a workspace-graph-only strict check silently did nothing unless `--strict` was also passed. Both commands now accept a real `--strict-graph` flag (`SnapshotArgs`/`TagArgs` gained a `strict_graph: bool` field) and pass `args.strict`/`args.strict_graph` through to `escalate` unconditionally, matching the sibling commands' semantics: `--strict` still escalates graph diagnostics too (per `escalate`'s own `strict || strict_graph` rule), and `--strict-graph` alone now escalates them as well.
