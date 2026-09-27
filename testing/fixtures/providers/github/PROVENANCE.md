# GitHub REST via `gh api --include`

Captured 2026-09-19 with gh 2.101.0 by `testing/refresh-provider-fixtures.sh`, authenticated as an ordinary user (the endpoints are public). Read-only GETs. Note `gh` prints its status line with a bare LF and every header line with CRLF; the fixtures keep those bytes.

Trimming applied to every file: `content-length`, `x-oauth-*` and `x-accepted-oauth-*` response headers removed (stale or account-specific); release bodies drop `body` (release notes) and keep only the first 2 entries of `assets`.

- `release-published.http`: `gh api --include repos/cli/cli/releases/latest` (v2.101.0). All other fields as served, including `tag_name`, `draft`, `prerelease`, `immutable`, `target_commitish` and per-asset `name`, `size`, `digest`, `state`.
- `release-draft.http`: DERIVED, not captured (a real draft needs write access). It is `release-published.http` with `.draft` set to `true`; nothing else changes (a real draft also has `published_at: null`).
- `release-prerelease.http`: DERIVED from `release-published.http` with `.prerelease` set to `true`.
- `release-404.http`: `gh api --include repos/cli/cli/releases/tags/callisto-no-such-tag`. Untouched apart from the header removals above; `gh` exits 1 for it, which the fixture does not record.
- `graphql-release-found.http`, `graphql-release-absent.http`, `graphql-release-repository-error.http`: `gh api --include graphql` with the release-by-tag query (`repository(owner, name) { release(tagName) { databaseId } }`) for `cli/cli` tag `v2.101.0`, `cli/cli` tag `callisto-no-such-tag`, and the missing repository `cli/callisto-no-such-repo-xyz`. Captured 2026-09-27. These also drop `date`, `etag`, `x-github-request-id` and `x-ratelimit-*`. `gh` exits 1 for the repository error, which the fixture does not record.
