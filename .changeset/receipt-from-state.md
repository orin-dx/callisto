---
callisto-cli: patch
---

**Release receipt no longer re-reads providers**

`release execute` builds the receipt from the exact evidence stored when each operation succeeded, so a flaky registry read after publishing can no longer fail a completed release. Execution state is now schema version 3; state files from earlier versions are rejected.
