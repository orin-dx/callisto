---
callisto-cli: patch
---

**Flit packages report their version.** A Flit `[tool.flit.metadata]` version is now read wherever a PEP 621 or Poetry version is, so a Flit project is no longer treated as having no version.
