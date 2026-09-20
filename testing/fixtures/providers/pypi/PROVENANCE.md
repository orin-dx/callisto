# PyPI JSON API

Captured 2026-09-19 with curl 8.7.1 by `testing/refresh-provider-fixtures.sh`. Read-only GETs, no credentials.

- `version-200.http`: `curl -sS --include https://pypi.org/pypi/requests/2.31.0/json`. Body reduced with jq to `info` (only name, version, summary, yanked, yanked_reason, requires_python), `last_serial` and the full `urls` array (2 files); `ownership` and `vulnerabilities` dropped; stale `content-length` header removed. Other headers are as served.
- `version-404.http`: same endpoint for the nonexistent project `requests-no-such-project-zz`. Untouched.
