---
callisto-graph: patch
---

`create_tags_with_options`'s preview branch (`permit: None`) and write branch (`permit: Some`) each independently computed `contains_tag` + `resolve_commit` + `RefNotFound` for an already-existing tag's actual sha. Extracted `resolve_existing_or_release_sha` as the single implementation, alongside the file's existing `plan_floating_major` helper -- which solved the identical preview/write divergence risk for the floating-major case. Both branches now call it instead of duplicating the resolution logic. No behavior change.
