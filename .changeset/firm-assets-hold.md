---
callisto-cli: patch
---

**`setup-callisto` with `verification: require` now fails when the release asset is missing.** It used to fall back to building from crates.io, so a job that asked for a verified prebuilt binary could run an unverified one.

- Why it matters: `require` now means the attested GitHub release asset or no install at all.
- The error names the asset and the URL it could not download, and also covers platforms with no prebuilt asset.
- Limits: `fallback` (the default) and `skip` still install from crates.io when the asset cannot be downloaded.

**Action:** if you use `verification: require` on a platform or release without a prebuilt asset, switch to `fallback` or pin a release that has one.
