# callisto-cli

Standalone command-line interface for Callisto monorepo versioning and release management.

## Overview

`callisto-cli` provides the primary command-line binary (`callisto`) for managing changesets, versions, pull requests, and releases:

- `callisto add`: Interactively or non-interactively record changesets.
- `callisto version`: Consume changesets, bump package versions, and update changelogs.
- `callisto status`: Inspect workspace release status.
- `callisto plan-publish`: Compute topological publish order.
- `callisto compose-pr-body`: Render rich GitHub Pull Request descriptions.

## License

Functional Source License, Version 1.1, MIT Future License (`FSL-1.1-MIT`).
