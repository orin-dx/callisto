---
callisto-cli: minor
---

**A Cargo dependency declared in several tables is followed and rewritten in each**

A dependency declared more than once, for example in `[dependencies]` and in a `[target.<cfg>.dependencies]` table with different constraints, now cascades to its dependent and is rewritten in every table when the package it names is released. Before, only the first declaration was updated and another one kept a stale constraint. A target declaration whose range still covers the new version now has its lower bound raised, as a plain declaration does. A Python requirement declared more than once, in the project dependencies, in several extras, in Poetry's table or under different markers, is followed and rewritten in each place by its own constraint, so a loose line such as `lib-py>=0.1` has its floor raised to `>=0.2.0` rather than being skipped.

Breaking:
- `callisto_model`: `DependencyEntry` and `DepEdge` gain a `locator: DepLocator` field naming the table a declaration lives in. `DepLocator` is `#[non_exhaustive]`: build it with `DepLocator::cargo` or `DepLocator::python`, or `DepLocator::default()` for npm. The new `DepContainer` names the Python table.
- `callisto_manifests`: `Manifest::update_dependency_spec` takes the declaration's `&DepLocator` after the dependency kind.
- `callisto_graph`: `RewriteKey` gains a `locator` field.
