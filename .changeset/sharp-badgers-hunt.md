---
callisto-cli: minor
---

**`callisto add` starts from what you changed**: packages changed since their last release are listed first, marked and already ticked, so a typical changeset is Enter, pick bumps, type a summary.

- Each answer stays on screen, the prompts show their keys, and long package lists page.
- Before writing, a review lists each package's bump and the summary; afterwards `add` prints the next step.
- Leave the summary empty to write a longer one, with bullets, in `$EDITOR`.
- Esc or Ctrl-C cancels at any question, prints `Cancelled, nothing written.` and leaves the cursor visible. Ctrl-C exits 130.
