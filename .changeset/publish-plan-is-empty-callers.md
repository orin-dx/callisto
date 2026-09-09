---
callisto-cli: patch
---

Two `callisto-cli` sites re-derived `PublishPlan` emptiness by hand instead of calling the canonical `PublishPlan::is_empty()` (`callisto-model`), which already ANDs all five plan fields specifically to avoid a hand-listed check silently missing one (e.g. `pypi_packages`):

- `commands/publish.rs`'s `write_dry_run_text` hand-listed all five fields with `&&`.
- `render/mod.rs`'s `render_publish` computed a `total_packages` sum of the four package-field lengths plus a separate `releases.is_empty()` check.

Both now call `plan.is_empty()`. No behavior change -- `render_publish` keeps `total_packages` for its package-section-skip branch and package counts, only the true-emptiness branch now calls `is_empty()`.
