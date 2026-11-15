---
callisto-cli: minor
---

`callisto version` now finishes an interrupted run when you run it again.

- Before, if a lockfile refresh failed (`E208`), a crash or Ctrl-C stopped it, or the decision or preview file could not be written, running `version` again was refused with `E121` and the half-applied changes had to be sorted out by hand.
- Now `version` records its inputs before its first write and removes the record when it finishes. Running it again after an interruption plans from those inputs and completes the run, with the same result as a run that was never interrupted.
- If `HEAD`, the tags, or a manifest, `callisto.toml` or a changeset the run depends on changed in a way the interrupted run did not cause, it stops with `E340` and changes nothing, and tells you to run `callisto version --abort`.
- `callisto version --abort` discards an interrupted run: it puts back the files the run wrote, removes the files it created, and leaves (and reports with `E342`) any file you edited since, so it can be repeated. It never touches a file the run did not write.
- The record is a git ref, `refs/worktree/callisto/checkpoint/version`. It is not shown by `git branch` or `git stash list` and not pushed by `git push --all` or `--tags`.
