---
callisto-cli: patch
---

status decides whether a package changed since its last tag by comparing its directories with the tagged ones, so it takes a fraction of a second however many commits the repository has, and a change that was later reverted no longer counts as a change.
