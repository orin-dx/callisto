---
callisto-cli: patch
---

**`callisto add` writes only changesets that `status` reads back as written**: a package name with a tab, no-break space or other whitespace at either end is now quoted, so its change is no longer lost.

- A package name with a line break fails with E268 before anything is written, instead of producing a file `status` cannot parse.
