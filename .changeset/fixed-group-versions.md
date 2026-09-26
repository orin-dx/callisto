---
callisto-cli: patch
---

Fixed groups compute their shared version from the highest member version when no member is tagged, support PEP 440 versions, and follow pre-release mode. Before, an untagged group could be bumped down to 0.x.
