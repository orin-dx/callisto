---
callisto-cli: patch
---

Waiting for npm packages to appear is now one window of confirm-timeout for the whole release: a package that shows up along the way no longer restarts it, and the time spent checking the registry counts, so a slow registry cannot stretch the wait to several windows.
