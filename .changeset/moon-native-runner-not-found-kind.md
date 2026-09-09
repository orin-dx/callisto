---
callisto-moon: patch
---

`MoonCommandRunner::run`'s native (non-`pdk`) path now classifies a spawn failure as `CommandError::NotFound` by checking the real `io::Error`'s `ErrorKind::NotFound` directly, instead of stringifying the error first and pattern-matching "not found"/"no such file" substrings in its `Display` text. That message-text heuristic (still used, and still needed, by the `pdk`/wasm path in `runner_pdk.rs`, whose host-exec errors carry no `ErrorKind` at all) made native "tool isn't installed" detection depend on OS-locale/libc wording rather than the type-checkable `ErrorKind` the standard library already provides for exactly this case -- a wording change or non-English locale could have silently downgraded `NotFound` to a generic `Io` error for callers that branch on `NotFound` for install-tool UX.
