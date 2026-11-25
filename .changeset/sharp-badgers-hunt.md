---
callisto-cli: minor
---

**`callisto add` starts from what you changed**: packages changed since their last release are listed first and marked `*`, so a typical changeset is Enter, pick bumps, type a summary.

- Package lists work as in `git add -i`: printed once, numbered, then you type numbers, ranges (`1-3`) or package names on one line, and Enter keeps the marked ones.
  - Names are shown in full, and nothing is redrawn, so any terminal size works.
  - An answer that names nothing in the list is asked again with the reason.
  - A number is a row only when that row exists, so a package named `42` is picked by typing `42`.
  - `init` asks for the product package the same way when several packages build binaries.
- Before writing, a review lists each package's bump and the summary; afterwards `add` prints the next step.
- Leave the summary empty to write a longer one, with bullets, in `$VISUAL` or `$EDITOR`. If that editor cannot start, `add` warns and asks for the summary inline, keeping your other answers.
- Esc or Ctrl-C at any `add` or `init` question cancels: it prints `Cancelled, nothing written.`, restores the terminal and exits 130.
  - A SIGINT sent to Callisto from outside at a question ends it with default signal behaviour; run `reset` if the terminal is left raw.
- If Callisto cannot tell which packages changed, it warns once and marks none instead of failing.
- With `--format json`, `add` and `init` never prompt: they ask for `--package` or `--yes` instead.
  - `init` prompts on the same terms: with stderr redirected (`callisto init 2>log`) it no longer asks, and fails with E240 unless you pass `--yes`.
