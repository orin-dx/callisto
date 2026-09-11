---
callisto-graph: minor
callisto-cli: minor
---

**Restructure the release PR body to group by change instead of by package, and drop redundant/misleading sections**

A single `.changeset/*.md` file can name several packages, and its summary was copied verbatim into every one of them (correct and desirable for each package's own standalone `CHANGELOG.md`) -- but the PR body then rendered that same text once per package it touched, producing bodies with the same paragraphs repeated multiple times for any release spanning a fixed/linked group or a multi-package changeset. A real 10-package release PR measured at 629 lines.

- `compose-pr-body`'s `### 📦 What's Changing` section (renamed from `Package Release Details`) now groups by the underlying change (a changeset file, a commit, a dependency/peer bump) and lists every package it affects in one block, instead of one full block per package. A package bumped only via a fixed/linked group with no direct change of its own is now named in a short note instead of getting an empty-looking collapsible.
- The whole section is wrapped in one collapsible, closed by default and open only when the release contains a major bump; each change's own block follows the same rule. A routine minor/patch release now renders as just the summary table, one note, and a single closed toggle.
- The redundant "#### Version Change" sub-heading is gone (already shown one line above in each block's `<summary>`), and the major-bump callout uses `[!WARNING]` instead of `[!IMPORTANT]`.
- The summary table's Bump column now includes a severity emoji (🔴/🟡/🟢) for at-a-glance scanning.
- The "Suggested PR Labels" line is removed: the label it named was never merely suggested (the release-PR script always applies it via `gh pr create --label`/`gh pr edit --add-label`), so the line was both redundant with GitHub's own label UI and factually misleading. `compose-pr-body`'s now-pointless `--label` flag is removed along with it (`ComposePrBodyArgs`/`PrBodyOptions` drop the `labels` field); the release-PR action script no longer passes `--label` to `compose-pr-body`, only to the real `gh pr create`/`gh pr edit` calls where it's actually applied.

No changes to changeset authoring, parsing, or per-package `CHANGELOG.md` output -- this is scoped entirely to the PR body, the one place the same fanned-out content is read together in a single sitting.
