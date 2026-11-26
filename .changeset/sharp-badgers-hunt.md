---
callisto-cli: minor
---

**`callisto add` starts from what you changed**: packages changed since their last release are listed first and marked `*`, so a typical changeset is Enter, pick bumps, type a summary.

- Package lists work as in `git add -i`: printed once, numbered, then you type numbers, ranges (`1-3`) or package names on one line, and Enter keeps the marked ones.
  - Names are shown in full, and nothing is redrawn, so any terminal size works.
  - An answer that names nothing in the list is asked again with the reason.
  - An exact package name wins over a row number.
  - When a listed name cannot be typed exactly, such as one with a space or comma, the list says to pick by row number and typed names are asked again.
  - `init` asks for the product package the same way when several packages build binaries, and accepts any name `--product-package` accepts, such as `tool` for `cargo/tool`.
- Before writing, a review lists each package's bump and the summary; afterwards `add` prints the next step.
- Leave the summary empty to write a longer one, with bullets, in `$VISUAL` or `$EDITOR`. If that editor cannot start, `add` warns and asks for the summary inline, keeping your other answers.
- Esc or Ctrl-C at any `add` or `init` question cancels: it prints `Cancelled, nothing written.`, restores the terminal and exits 130.
  - A SIGINT sent to Callisto from outside at a question ends it with default signal behaviour; run `reset` if the terminal is left raw.
- If Callisto cannot tell which packages changed, it warns once and marks none instead of failing.

**Breaking:** `add` and `init` prompt only when stdin and stderr are terminals and the format is text. Before, a terminal on stdin was enough, so `callisto add 2>log` or `callisto --format json add` opened the wizard; now `add` exits 2 with E214 and `init` with E240. Pass `--package` and `--summary` to `add`, and `--yes` to `init`, in scripts.
