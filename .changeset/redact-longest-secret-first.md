---
callisto-cli: patch
---

A credential that contains another credential is now redacted whole from command output.

- Before, secrets were replaced one after another in the order they were found, so if a shorter secret was replaced first (for example `abcd` inside `abcdefgh`), the rest of the longer one stayed visible in error output.
- Now the longest secret is replaced first.
- You do not need to do anything.
