---
callisto-graph: patch
---

Fixed groups now compute their shared version like any other bump: an untagged group aligns on its highest on-disk member version instead of 0.0.0, a missing tagged member's base version is an error instead of silently defaulting to 1.0.0, PEP 440 groups no longer fail with a SemVer error, and pre-release mode is honored.
