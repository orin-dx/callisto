---
callisto-cli: patch
---

`version --emit-preview` is faster in large monorepos.

- The check for packages that changed since their last release now asks git once for all packages tagged at the same release, instead of once per package.
- In a test workspace with 150 changed packages, that went from 151 git calls to 4, and from 3.8 s to 1.0 s.
