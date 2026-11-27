---
callisto-cli: patch
---

**A closed stdout, or a stderr that only carries notes, no longer changes how a command exits**

Piping into a command that stops reading early (`callisto status | head`) or running with stderr closed now leaves the exit status as it would be otherwise.

- Every command, `callisto help`, `--help` and `--version` included, treats a closed stdout as the end of its output in text and JSON alike, instead of failing with E213.
- Notes, warnings, a publishing tool's echoed output and the final error card are best effort: a closed stderr no longer crashes `callisto` with exit 101, a usage error still exits 2, and a refused release keeps its own error code.
- Output a command needs on stderr still fails it when it cannot be written: `add`'s wizard prompts, and `init`'s review of what it detected and will write, which goes to stderr under `--format json`.
- A file `callisto` must write, such as a changeset or a receipt, still fails the command when it cannot be written.
