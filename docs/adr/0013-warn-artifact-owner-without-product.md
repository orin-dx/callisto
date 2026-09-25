# 13. Warn when an artifact's owner is selected without its product

## Context

A `[[release.artifact]]` slot's `package` (the asset's build owner) need not be the product package. Derivation only builds artifact slots while iterating the product's own selected entry, so when the owner is selected for release but the product is not, that iteration never runs and the asset is dropped with no signal at all.

## Decision

Derivation checks whether the product is selected. If it isn't, and an artifact's owner package is selected on its own, Callisto prints a warning naming the owner, the product and the asset, and still derives no slot for it.

## Consequences

- The run still succeeds; a standalone owner release is not blocked by a product that isn't releasing this time.
- The asset is never silently missing from a release without any trace in the output.
