---
callisto-cli: patch
---

A mistyped package name now gets a suggestion, so you can fix it from the error alone.

- `callisto add --package cargo/libb:minor` adds ``did you mean `cargo/lib`?`` to the E102 error, with up to three close matches.
- A wrong ecosystem is caught too: `npm/lib` suggests `cargo/lib`.
- When nothing is close, the error is unchanged.
