---
callisto-cli: minor
---

Usage errors now exit 2, as the stability promise says, so a script can tell a mistake in the command from a failure in the workspace.

- A bad or missing flag value that Callisto checks after parsing now exits 2, like a clap-level mistake always did. Examples: `add --package a:patch`, `add --package cargo/a:bogus`, `add` without `--summary`, `init` without `--yes` and without a terminal, `snapshot --tag` with a space, and `--dry-run` on commands that need an output file.
- Failures about your workspace, config, git or registries still exit 1.
- The JSON error envelope is unchanged.

Action: a script that checks for exit code 1 after a bad argument must also accept 2. Checking for any non-zero code is unaffected.
