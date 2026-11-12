---
callisto-cli: patch
---

Python dependencies written as `name (==1.0.0)` are now recognized, so bumping that dependency releases its dependents and updates the pin.

- Before, the legacy parenthesized form was not seen as a dependency: the dependent was not released and its pin stayed old. The same pin written `name==1.0.0` already worked.
- Now a bump of `name` to 2.0.0 patch-releases the dependent and rewrites the pin to `name (==2.0.0)`, keeping the parentheses.
- A requirement like `name[extra] @ https://...` is now read with the URL kept out of the extras.
- You do not need to do anything. If you had worked around this by rewriting pins to `name==1.0.0`, you can leave them.
