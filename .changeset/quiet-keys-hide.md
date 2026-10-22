---
callisto-cli: patch
---

An error from a failing `git` or other helper command run while versioning no longer prints a credential that the command echoed, such as a token in an authenticated remote URL. Registry tool output shown in errors and the CI log also hides more credential shapes: `npm_`, `ghp_` and `github_pat_` tokens, PyPI API tokens, `Bearer` values and `.npmrc` auth lines, and the value of any environment variable named like `*_TOKEN`, `*_SECRET` or `*_PASSWORD`, even when it is not one of the variables Callisto reads itself.
