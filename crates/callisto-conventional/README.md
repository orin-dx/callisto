# callisto-conventional

Conventional Commits specification parser and severity classifier for Callisto.

## Overview

`callisto-conventional` parses Git commit messages following the Conventional Commits specification:

- Parsing commit types (`feat`, `fix`, `docs`, `refactor`, `chore`, etc.).
- Extracting breaking change footers (`BREAKING CHANGE:`).
- Classifying semver bump severity (`major`, `minor`, `patch`) from commit history.

## License

Functional Source License, Version 1.1, MIT Future License (`FSL-1.1-MIT`).
