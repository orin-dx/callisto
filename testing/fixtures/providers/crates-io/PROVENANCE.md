# crates.io sparse index

Captured 2026-09-19 (server `date` headers read 2026-09-20 UTC) with curl 8.7.1 by `testing/refresh-provider-fixtures.sh`. Read-only GETs, no credentials.

- `sparse-index-200.http`: `curl -sS --include https://index.crates.io/ca/ll/callisto-model`. Body trimmed to its first 3 lines (versions 0.3.0, 0.3.1, 0.3.2); the stale `content-length` header is removed; everything else is byte-for-byte as served.
- `sparse-index-404.http`: same command for the nonexistent crate `callisto-model-no-such-crate-zz`. Untouched (S3 `NoSuchKey` XML body).
- `sparse-index-yanked-line.json`: the single line for `chacha20` 0.10.1 from `https://index.crates.io/ch/ac/chacha20` (`grep -F '"vers":"0.10.1"'`); the script fails if its `yanked` is not `true`. No headers.
