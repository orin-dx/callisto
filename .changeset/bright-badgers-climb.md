---
callisto-cli: patch
---

After publishing, Callisto looks for a new npm version right away and then at 15 seconds, 45 seconds, 2 minutes and 3 minutes, instead of every 30 seconds, so a release makes about five looks per window rather than seventeen. The window and `confirm-timeout` are unchanged.
