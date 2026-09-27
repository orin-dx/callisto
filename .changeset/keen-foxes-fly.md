---
callisto-cli: minor
---

Adding a package with the same name in another ecosystem needs no configuration: the existing package keeps its release history, tags and changelog, and new tags use the qualified name (`cargo/foo@1.1.0`). A pre-release cycle started before the new package keeps working. A changeset that names a now-ambiguous `foo` fails with E103 and shows the qualified form to write.
