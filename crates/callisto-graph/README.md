# callisto-graph

Polyglot dependency DAG solver, cascade propagator, and publish planner for Callisto.

## Overview

`callisto-graph` constructs and solves dependency graphs for polyglot monorepos:

- Topological sorting of workspace packages for publication order.
- Dependency cascade resolution (propagating major/minor bumps downstream).
- Release plan derivation (`plan_local_release`, `build_release_intent`) and PR description formatting (`compose-pr-body`).

## License

Functional Source License, Version 1.1, MIT Future License (`FSL-1.1-MIT`).
