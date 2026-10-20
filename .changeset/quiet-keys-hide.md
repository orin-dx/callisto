---
callisto-cli: patch
---

Registry tool output shown in errors and the CI log now hides more credential shapes: `npm_`, `ghp_` and `github_pat_` tokens, PyPI API tokens, `Bearer` values and `.npmrc` auth lines, and the value of any environment variable named like `*_TOKEN`, `*_SECRET` or `*_PASSWORD`, even when it is not one of the variables Callisto reads itself.
