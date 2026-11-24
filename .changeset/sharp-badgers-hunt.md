---
callisto-cli: minor
---

**`callisto add` starts from what you changed**: packages changed since their last release are listed first, marked and already ticked, so a typical changeset is Enter, pick bumps, type a summary.

- Each answer stays on screen, the prompts show their keys, and long package lists page.
- Before writing, a review lists each package's bump and the summary; afterwards `add` prints the next step.
- Leave the summary empty to write a longer one, with bullets, in `$VISUAL` or `$EDITOR`. If that editor cannot start, `add` warns and asks for the summary inline, keeping your other answers.
- Esc or Ctrl-C at any `add` or `init` question cancels: it prints `Cancelled, nothing written.`, restores the terminal and exits 130.
  - A SIGINT sent to Callisto from outside at a question ends it with default signal behaviour; run `reset` if the terminal is left raw.
- If Callisto cannot tell which packages changed, it warns once and lists every package unticked instead of failing.
- With `--format json`, `add` and `init` never prompt: they ask for `--package` or `--yes` instead.
  - `init` prompts on the same terms: with stderr redirected (`callisto init 2>log`) it no longer asks, and fails with E240 unless you pass `--yes`.
