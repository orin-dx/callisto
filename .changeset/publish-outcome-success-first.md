---
callisto-cli: patch
---

A successful `cargo publish` or `npm publish` is no longer reported as already published because its output happens to contain "already exists" or the letters `e409`. A failed `npm publish` counts as already published only when npm reports the `E409` or `EPUBLISHCONFLICT` code or the "cannot publish over the previously published versions" error.
