---
callisto-manifests: patch
---

`CargoToml::publish_targets()` was a byte-for-byte duplicate of the `Manifest` trait's own default `publish_targets()` implementation -- `CargoToml::ecosystem()` already returns `Ecosystem::Cargo`, so the default impl dispatches to the identical `CratesIo`/`None` output without an override. Deleted the redundant override; added a direct regression test (`publish_targets_uses_trait_default_dispatch_for_cargo`) pinning that the trait-default dispatch still produces the same output for both publishable and `publish = false` manifests. No behavior change.
