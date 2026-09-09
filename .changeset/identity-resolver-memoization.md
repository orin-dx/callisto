---
callisto-graph: patch
callisto-moon: patch
---

`IdentityResolver::resolve` did a fresh `fs::read_to_string` + full manifest parse on every call with no memoization. `MoonProjectLocator::declared_edges` re-resolved every project's identity a second time (`projects()`, called earlier in the same flow, already resolved it once) and, worse, re-resolved a dependency edge's target once per edge rather than once per unique target project -- a widely-depended-on package (e.g. a shared crate with 30 internal dependents) had its manifest read and parsed 30+ times in a single `Workspace::load`.

`IdentityResolver` now memoizes `resolve`'s result by `(path, ecosystem)` for its whole lifetime. Since `MoonProjectLocator` holds one `IdentityResolver` across both `projects()` and `declared_edges()`, a given manifest is now read and parsed at most once per run, with no change needed to `declared_edges` itself.
