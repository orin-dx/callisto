---
callisto-cli: minor
---

**PyPI publish targets work in durable releases**

`callisto release plan` no longer refuses PyPI. Versions are observed through the PEP 691 JSON simple index (`https://pypi.org/simple/<project>/`, or the configured private index) instead of `pip`, which cannot tell a missing project from an unreachable index. A yanked file reads as absent, so publishing over a yanked version fails closed. A private index that serves only PEP 503 HTML is reported as indeterminate; it must support the JSON simple index. Requires `curl` on the release runner.
