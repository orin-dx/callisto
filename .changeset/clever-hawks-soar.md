---
callisto-cli: patch
---

**`release-pr` JSON arguments report where a failure came from.** A relative `--decision` or `--snapshot` file is now read relative to `--cwd`, and a bad file no longer looks like a usage mistake.

- Malformed inline or stdin JSON is still `E281`, exit 2, naming the flag.
- A malformed file is `E238`, exit 1, naming the file; an unreadable file is `E213`.
- `compose-pr-body --preview` follows the same rule: a malformed preview file is `E238`, exit 1, instead of `E281`.
- **Scripting:** treat exit 1 from a file argument as a data problem, not a bad invocation.
