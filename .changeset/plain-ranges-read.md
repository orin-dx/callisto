---
callisto-cli: patch
---

An npm dependency range written with spaces between its comparators, such as `>=1.0.0 <2.0.0` or `1.0.0 - 2.0.0`, is now read as a range. When a release takes its dependency out of it, `version` stops with E319 before writing anything and tells you to edit the range. Before, such a range was skipped silently, so the dependent was neither updated nor released and kept declaring compatibility with the old version.
