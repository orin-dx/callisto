---
callisto-cli: minor
---

Usage errors now exit 2, as the stability promise says, so a script can tell a mistake in the command from a failure in the workspace.

- A bad or missing flag value that Callisto checks after parsing now exits 2, like a clap-level mistake always did. Examples: `add --package a:patch`, `add --package cargo/a:bogus`, `add` without `--summary`, `init` without `--yes` and without a terminal, `snapshot --tag` with a space, and `--dry-run` on commands that need an output file.
- Also exit 2: an unknown package passed to `--package` on `add`, `matrix` or `release` (E102), an invalid `init --forge-repository` (E187) or `--product-package` (E189), a `release pending --from` that is not a release commit (E328), a repeated `--package` in a release selection (E169), an invalid `--decision`, `--preview` or `--snapshot` value (E281), `release execute` flags that contradict the intent (E232, E233), and a `release plan --artifact-repository` that does not match the config (E230).
- The rule: exit 2 when a flag value or combination is rejected, exit 1 when a valid flag meets state that cannot support it. Failures about your workspace, config, git, registries or release intent (for example E188, E299, E219, E308) still exit 1, and so does the same value read from `callisto.toml`, a changeset or `pre.json`.
- The JSON error envelope is unchanged.

Action: a script that checks for exit code 1 after a bad argument must also accept 2. Checking for any non-zero code is unaffected.
