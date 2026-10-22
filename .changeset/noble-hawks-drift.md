---
callisto-cli: patch
---

Inference from commits now stops with E315 in a shallow clone instead of planning a smaller bump from the history it can see. A workspace table written inline or with dotted keys is recognised as a Cargo workspace root, and a pnpm workspace is published with pnpm even where its lockfile is not committed. An unreadable `src/bin` no longer hides a binary from `callisto init`, and the napi target check reads the main package's own `package.json` wherever it is.
