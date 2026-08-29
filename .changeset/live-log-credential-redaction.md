---
callisto-cli: patch
---

**Fix: credential redaction now covers the live-streamed CI log, not just the captured error**

`callisto publish` streams a registry command's stderr to the terminal in real time as it runs, separately from the captured copy redacted afterward for the final error message. A credential embedded in that stderr (e.g. a private registry URL with basic auth) was previously redacted only in the captured copy -- the live stream, which a CI log persists, was not. Both are now redacted identically; the captured copy stays raw internally so error classification (rate-limit, auth-failure detection) still works on the exact upstream text.
