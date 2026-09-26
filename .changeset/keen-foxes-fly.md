---
callisto-cli: minor
---

Adding a package with the same name in another ecosystem no longer changes the existing package's tags: default tags are `{name}@{version}` from the manifest name, so a Cargo `foo` keeps finding `foo@1.0.0` after it is shown as `cargo/foo`. Two packages that would share default tags now fail with E101; give all but one a `tag-template`. An already-promoted package without a `tag-template` previously tagged `cargo/foo@{version}`; set `tag-template = "cargo/foo@{version}"` to keep that.
