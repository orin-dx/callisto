---
callisto-cli: minor
---

`callisto version` now finishes an interrupted run when you run it again.

- Before, if a lockfile refresh failed (`E208`), a crash or Ctrl-C stopped it, or the decision or preview file could not be written, running `version` again was refused with `E121` and the half-applied changes had to be sorted out by hand.
- Now `version` records its inputs before its first write and removes the record when it finishes. Running it again after an interruption plans from those inputs and completes the run, with the same result as a run that was never interrupted.
- If `HEAD`, the tags, or a manifest, `callisto.toml` or a changeset the run depends on changed in a way the interrupted run did not cause, it stops with `E340` and changes nothing, and prints the commands that undo the interrupted run. Those commands touch only the files the run wrote.
- The record is a git ref, `refs/callisto/checkpoint/version`. It is not shown by `git branch` or `git stash list` and not pushed by `git push --all` or `--tags`.
