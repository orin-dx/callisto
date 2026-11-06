---
callisto-cli: patch
---

A release that publishes several npm packages no longer fails with E157 on a slow registry just because earlier publishes took long. Each publish now gets its full confirmation window from when it was issued, where before all of them shared one window that started at the first.
