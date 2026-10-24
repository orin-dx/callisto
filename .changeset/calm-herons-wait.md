---
callisto-cli: patch
---

A release no longer waits past its confirmation window when a registry keeps asking to retry later. The run now gives up when the window ends, as the error already said.
