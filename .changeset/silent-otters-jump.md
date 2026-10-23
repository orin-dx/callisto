---
callisto-cli: patch
---

status takes about a third of the time on a repository with tens of thousands of commits: the history is read by several git processes at once.
