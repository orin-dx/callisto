---
callisto-cli: patch
---

**Faster failure on unreachable registries**

Registry checks disable cargo's and npm's own retries and rely on Callisto's bounded retry. An unreachable registry now reports in seconds instead of 1–6 minutes.
