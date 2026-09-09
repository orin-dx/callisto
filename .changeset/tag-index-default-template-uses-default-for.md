---
callisto-graph: patch
---

**Fix `TagIndex::build` re-deriving a package's default tag template instead of reusing `TagTemplate::default_for`**

`TagIndex::build`'s per-package loop built each package's default tag template by hand-formatting `"{name}@{version}"` and re-parsing it via `TagTemplate::parse`, instead of calling `callisto_model::TagTemplate::default_for` -- the single source of truth for this exact value already used by `release.rs`'s own tag-name construction (`unwrap_or_else(|| callisto_model::TagTemplate::default_for(&package.id))`). The two implementations behaved differently: `TagTemplate::parse` additionally validates git-ref-name legality, so `TagIndex::build` could reject a package name that `release.rs`'s path would accept unchecked -- genuine behavioral divergence between two code paths computing the same value.

`TagIndex::build` now calls `TagTemplate::default_for` directly, matching `release.rs`. **Behavior change**: this intentionally removes the extra git-ref-name validation `TagTemplate::parse` was performing on this call site's default-template path. This is not a silent regression -- it aligns `tags.rs` with the already-shipped `default_for` behavior `release.rs` relies on, rather than adding a new validation requirement to `default_for` itself (which would affect every caller). A custom `tag_template` configured via `[[package-set]]` still goes through `TagTemplate::parse` and its full validation, unchanged.
