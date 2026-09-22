---
callisto-cli: minor
---

**PyPI publish targets work in durable releases**

Versions are checked via the PEP 691 JSON simple index instead of `pip`; yanked versions count as absent. Private indexes must serve the JSON simple index. Requires `curl` on the release runner.
