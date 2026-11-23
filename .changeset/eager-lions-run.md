---
callisto-cli: patch
---

**setup-callisto `verification: require` no longer falls back to crates.io**

A missing or undownloadable release asset now fails the step instead of installing an unverified build.

- Why it matters: `require` now means the installed binary was verified.
- How to adopt: nothing to change; use `verification: fallback` to keep the crates.io fallback.
