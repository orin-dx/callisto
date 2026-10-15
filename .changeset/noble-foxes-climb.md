---
callisto-cli: minor
---

**A Cargo dependency declared in several tables is followed and rewritten in each**

A dependency declared more than once, for example in `[dependencies]` and in a `[target.<cfg>.dependencies]` table with different constraints, now cascades to its dependent and is rewritten in every table when the package it names is released. Before, only the first declaration was updated and another one kept a stale constraint. A target declaration whose range still covers the new version now has its lower bound raised, as a plain declaration does. A Python requirement declared twice with different constraints, in two extras or with two markers, is still followed by its first declaration only.

Breaking:
- `callisto_model`: `DependencyEntry` and `DepEdge` gain a `locator: DepLocator` field naming the table a declaration lives in. `DepLocator::default()` is the value for npm and Python declarations.
- `callisto_manifests`: `Manifest::update_dependency_spec` takes the declaration's `&DepLocator` after the dependency kind.
- `callisto_graph`: `RewriteKey` gains a `locator` field.
