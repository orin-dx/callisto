---
callisto-cli: patch
---

A Poetry caret or tilde dependency on a Python package released together with it, such as `core-py = "^0.1.0"`, is now rewritten to cover the new version instead of being left behind with a warning. A range that spans a single minor, such as `core-py>=0.1.0,<0.2`, stays one minor wide (`>=0.2.0, <0.3`) instead of widening to the next major.
