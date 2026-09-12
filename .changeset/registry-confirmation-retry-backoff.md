---
callisto-graph: patch
---

**Retry the registry-publish confirmation check with backoff instead of failing on the first miss**

The registry-publish confirmation added for Cargo and already used for npm checked the registry exactly once. Propagation lag is normally sub-second, but a single check right after a publish could still land inside that window and fail immediately with `RegistryPublishUnconfirmed`, even though a moment later it would have succeeded on its own.

The confirmation check now retries up to 3 times (4 checks total) with exponential backoff (2s, 4s, 8s -- 14s worst case) before reporting unconfirmed. Only registry propagation lag is retried this way; a hard error from the check itself is never retried.
