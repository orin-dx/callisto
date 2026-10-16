---
callisto-cli: patch
---

A package keeps its release history when its directory moves, when it gains a higher-priority manifest beside a same-named package, or when a same-named sibling is removed. Before, the last release was reported as none, or an older release was chosen over a newer one, so already-released work could be released again.
