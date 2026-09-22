---
callisto-cli: patch
---

**Unreachable registries are reported in seconds, not minutes**

Registry observation now turns off the package manager's own retries (`cargo --config net.retry=0 info`, `npm view --fetch-retries=0`) and relies on Callisto's bounded retry alone. Previously each of Callisto's attempts also retried inside cargo or npm, so an unreachable registry took about a minute (cargo) or six minutes (npm) before reporting indeterminate.
