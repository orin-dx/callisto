---
callisto-graph: patch
---

Fixed/linked group severity computation ("max severity across a group, propagate to every member") was independently re-derived in four places: `aggregate::union_fixed`/`union_linked`, two inline blocks in `cascade::solve_cascade`, and `groups::fixed_group_target` (which redundantly recomputed a value its caller had already computed). Only `aggregate`'s copies guarded against a group member removed from the workspace; `cascade`'s Linked-group block had no such guard, so a stale member reachable through a live sibling's severity bump during cascade convergence could hit a `MissingField` crash.

Added `GroupDef::package_members()` and `GroupDef::max_severity()` as the single definitions of these two operations, used by all four call sites. `cascade::solve_cascade`'s Linked-group block now applies the same stale-member guard `aggregate` already had. `fixed_group_target` now takes the caller's already-computed severity instead of recomputing it. Also deleted `CascadeSolver`, a trait with zero implementations and zero call sites (its own doc comment noted no implementation exists; the free function is called directly everywhere).
