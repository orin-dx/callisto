---
callisto-model: patch
---

`PackageId::parse`'s `:`-prefixed and bare-with-slash branches independently ran the same three-check validation sequence on the post-separator remainder (empty-after-prefix, leading-hyphen, path-traversal) before constructing an identical `PackageId::Prefixed`. The `LeadingHyphen` check had already been hand-added to both copies separately in one prior commit.

Extracted `PackageId::validate_prefixed_remainder` as the single implementation, called from both branches. No behavior change -- error variants, field values, and messages are unchanged for every existing case.
