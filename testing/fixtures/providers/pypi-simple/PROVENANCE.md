# PyPI simple index via `curl -sS -i`

Captured 2026-09-22 with curl against the real `https://pypi.org/simple/` index, `Accept: application/vnd.pypi.simple.v1+json` (PEP 691). Read-only GETs against public projects. `curl -i` writes the status line and headers with CRLF and joins them to the body with a blank line, same framing `parse_http_response` expects.

Trimming applied to every file: `date`, `etag`, `x-served-by`, `x-cache`, `x-cache-hits`, `x-timer`, `x-pypi-last-serial` and `content-length` headers removed (stale, cache-node- or serial-specific); nothing else changed, including exact `files[]` order and content.

- `found.http`: `curl -sS -i -H "Accept: application/vnd.pypi.simple.v1+json" https://pypi.org/simple/iniconfig/`. No file is yanked; `iniconfig-2.3.0.tar.gz` and `iniconfig-2.3.0-py3-none-any.whl` are the exact-match fixture for version `2.3.0`.
- `yanked.http`: `curl -sS -i -H "Accept: application/vnd.pypi.simple.v1+json" https://pypi.org/simple/pluggy/`. Both `pluggy-1.1.0.tar.gz` and `pluggy-1.1.0-py3-none-any.whl` carry a non-empty `yanked` reason string (PyPI yanks all files of a release together); version `1.1.0` is the yanked-match fixture. Also exercises a legacy `.zip` sdist (`pluggy-0.4.0.zip`) and PEP 440 dev-release filenames (`pluggy-1.0.0.dev0*`).
- `absent.http`: `curl -sS -i -H "Accept: application/vnd.pypi.simple.v1+json" https://pypi.org/simple/callisto-definitely-not-a-real-package-xyz-404/`. A real 404, plain-text body, no `files[]` to parse.
