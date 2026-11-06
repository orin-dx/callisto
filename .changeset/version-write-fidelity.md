---
callisto-cli: patch
---

`callisto version` no longer drops the BOM or converts CRLF to LF in a Cargo workspace root manifest when it rewrites a workspace dependency. A changelog that only quotes a version heading (indented, or in a code block) no longer makes the release skip that version's entry. A package whose display name gains an ecosystem prefix keeps one title in its changelog. An npm `overrides` or `resolutions` value that is `$name` or has a protocol (`npm:`, `file:`, `catalog:`) is left as written.
