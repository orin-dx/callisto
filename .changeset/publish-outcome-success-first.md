---
callisto-cli: patch
---

A successful `cargo publish` or `npm publish` is no longer reported as already published, and a failed `npm publish` counts as already published only when npm reports a version conflict, such as `E409` or `EPUBLISHCONFLICT`.
