---
callisto-cli: patch
---

A successful `cargo publish` or `npm publish` is no longer reported as already published because its output happens to contain "already exists" or the letters `e409` (npm prints a hash that can). On a failure the npm conflict codes must be whole words.
