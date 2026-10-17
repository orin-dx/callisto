---
callisto-cli: patch
---

A file Callisto cannot read or look up is now reported instead of treated as missing. An unreadable package manifest, `callisto.toml`, `pre.json` or changelog, a changeset it cannot inspect, and a failed directory sync after a write now stop the command with the file named, where before the package, config, changeset or durability guarantee was silently dropped.
