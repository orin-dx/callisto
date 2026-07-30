# callisto-graph

Polyglot dependency DAG solver, cascade propagator, and publish planner for Callisto.

## Overview

`callisto-graph` constructs and solves dependency graphs for polyglot monorepos:

- Topological sorting of workspace packages for publication order.
- Dependency cascade resolution (propagating major/minor bumps downstream).
- Release plan composition (`plan-publish`) and PR description formatting (`compose-pr-body`).

## License

GNU Affero General Public License v3.0 (`AGPL-3.0-only`).
