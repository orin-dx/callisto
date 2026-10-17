---
callisto-cli: patch
---

`version` now checks every file it is about to write before it writes the first one. A manifest that is missing, unreadable, a directory, in an unwritable folder, or reached through a symbolic link Git cannot stage stops the command with every such file named and nothing changed, instead of failing partway through and leaving some manifests already bumped.
