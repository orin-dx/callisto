# GitHub REST via `gh api --include`

Captured 2026-09-19 with gh 2.101.0 by `testing/refresh-provider-fixtures.sh`, authenticated as an ordinary user (the endpoints are public). Read-only GETs. Note `gh` prints its status line with a bare LF and every header line with CRLF; the fixtures keep those bytes.

Trimming applied to every file: `content-length`, `x-oauth-*` and `x-accepted-oauth-*` response headers removed (stale or account-specific); release bodies drop `body` (release notes) and keep only the first 2 entries of `assets`.

- `release-published.http`: `gh api --include repos/cli/cli/releases/latest` (v2.101.0). All other fields as served, including `tag_name`, `draft`, `prerelease`, `immutable`, `target_commitish` and per-asset `name`, `size`, `digest`, `state`.
- `release-draft.http`: DERIVED, not captured (a real draft needs write access). It is `release-published.http` with `.draft` set to `true`; nothing else changes (a real draft also has `published_at: null`).
- `release-prerelease.http`: DERIVED from `release-published.http` with `.prerelease` set to `true`.
- `release-list-page.http`: `gh api --include 'repos/cli/cli/releases?per_page=2&page=1'`, with the same per-release trimming; the `Link` header is as served.
- `release-404.http`: `gh api --include repos/cli/cli/releases/tags/callisto-no-such-tag`. Untouched apart from the header removals above; `gh` exits 1 for it, which the fixture does not record.
