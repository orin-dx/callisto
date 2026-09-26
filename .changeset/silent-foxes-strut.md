---
callisto-cli: patch
---

A malformed release intent file (duplicate operation, cycle, mismatched artifact roster, digest mismatch, or unsupported trust profile) reports its own specific error code from release verify and release execute, instead of one generic code for every case.
