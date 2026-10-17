---
callisto-cli: patch
---

Generated release workflows now install the Python build and upload tools they need. A workspace that publishes to PyPI no longer depends on `twine` being preinstalled on the runner.
