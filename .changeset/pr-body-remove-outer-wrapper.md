---
callisto-graph: patch
---

**Stop hiding the release PR's change list behind an extra collapsible**

The outer `<details>` wrapper added around `### 📦 What's Changing` (closed by default unless a major bump is present) made the entire list of what changed invisible until clicked — including for a routine minor-only release, where a reviewer had to open the wrapper just to see the *names* of the changes before deciding whether to read any of them.

The outer wrapper is removed; the list of changes (each one's own `<summary>` naming the change and the packages it affects) is now always directly visible, replaced only by a plain `**N change(s) across M package(s):**` count line. Each individual change's own body still collapses by default and opens automatically when it contains a major bump — only the actual prose text collapses, never the fact that a change exists.
