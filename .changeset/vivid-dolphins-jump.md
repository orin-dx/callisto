---
callisto-cli: patch
---

status and version no longer start a git process for every release when packages were released at different times: the history after all of them is read once, so a workspace of 500 independently released packages takes a fraction of a second instead of over ten.
