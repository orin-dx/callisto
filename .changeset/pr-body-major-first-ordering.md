---
callisto-graph: patch
---

**Order the release PR's summary table and change list by severity, major first**

A reviewer merging a release PR is really answering one question: is anything breaking? Before this change, both the summary table and the "What's Changing" list rendered in plan order (roughly topological/declaration order), so a major bump could land anywhere among a dozen rows — a reviewer had to scan the whole table to be sure nothing was breaking, even though a major bump's own detail block already auto-opens.

Both the table and the change list are now sorted by severity descending (major, then minor, then patch/none), stable within each tier so otherwise-equal rows keep their original relative order. A breaking change is now always the first thing visible, in both places, regardless of where its package name or declaration would otherwise have put it.
