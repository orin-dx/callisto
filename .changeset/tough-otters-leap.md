---
callisto-cli: patch
---

A GitHub release that is not yet visible right after Callisto creates, uploads to or publishes it no longer fails the release with E167. Callisto now waits up to about a minute for it, and finds draft releases by tag directly instead of scanning the release list.
