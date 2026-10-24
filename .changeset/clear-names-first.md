---
callisto-cli: patch
---

A first release of a package whose name another project already owns is now refused with a clear error (E322), naming the repository the registry records for it, where before `release` reported the version as already published and told you to check out an old commit. This applies to a package this repository has never tagged; tag the version commit by hand if the package really is yours.
