---
callisto-cli: minor
---

**`init` and `release --dry-run` say less and say what to do next**

`init` prints each note once and a one-line summary of the generated workflow instead of its whole text, and `release --dry-run` now points the way when it cannot show what you expect.

- `init` prints each note and piece of advice once on stderr, with `--format json` too, where the report also lists them; in an interactive run the notes come before the write question and a decline keeps them.
- `init` names the workflow file, the branch it runs on and what it does, instead of printing the file.
- `init --workflow` in an already configured workspace says it wrote the workflow instead of claiming it initialized the configuration.
- `release --dry-run` says to run `callisto version` (or merge the release PR) first while it has work for the packages being released, including when there is nothing to release yet.
  - With `--package`, only work for the selected packages and their groups counts.
  - In a prerelease cycle, notes `callisto version` already applied do not count.
- An `origin` that is not a hosted remote, such as a local path or a `file://` URL, previews like no `origin`: the dry run shows the plan with one note that tags are unbound.
  - A real `callisto release` refuses it with an error that names the accepted forms: an `https://` or `ssh://git@` URL, or `git@host:owner/repo`.
  - The error never shows the `origin` value; run `git remote get-url --push origin` to see it.
- `init` accepts a non-hosted `origin` the same way, since it never pushes, and says once that it is not hosted.
