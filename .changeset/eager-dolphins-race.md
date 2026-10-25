---
callisto-cli: patch
---

Publishing a yarn package whose `package.json` sets `publishConfig.registry` no longer fails with "Unsupported option name (--registry)": yarn reads that setting itself, so Callisto no longer passes the flag.
