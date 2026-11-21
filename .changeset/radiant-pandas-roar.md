---
callisto-cli: patch
---

**`pre enter` and `pre exit` stage only their own `pre.json`.** A changesets directory whose name contains glob characters, such as `notes*`, no longer stages a neighbouring tracked `notes-other/pre.json` as well.
