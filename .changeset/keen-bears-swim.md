---
callisto-cli: patch
---

A snapshot tag that is empty or has a character a version cannot hold, such as a space or `/`, now stops with E324 and a fix. Before, it failed as an internal error, or an empty tag produced a version with a doubled dash.
