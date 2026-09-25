# 0001. Changesets file format

## Context

Teams adopting callisto often already use `@changesets/cli`, and many JS teams prefer written changesets over Conventional Commits.

## Decision

`.changeset/*.md` files are byte-compatible with `@changesets/cli`: YAML frontmatter mapping package names to `none`/`patch`/`minor`/`major`, then a Markdown summary.

## Consequences

- Adopting or leaving callisto is one commit; existing changesets keep working.
- Commit-based inference is opt-in per package (`release-trigger = "auto"`), never the default.
