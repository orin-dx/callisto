---
callisto-cli: patch
---

A release whose changelog exists but cannot be read now fails planning with E125 naming the file, instead of publishing generated notes in place of the changelog the release PR reviewed. A missing changelog, a missing version section and an empty section still fall back to generated notes, with the notice.
