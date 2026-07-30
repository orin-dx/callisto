# callisto-conventional

Conventional Commits specification parser and severity classifier for Callisto.

## Overview

`callisto-conventional` parses Git commit messages following the Conventional Commits specification:

- Parsing commit types (`feat`, `fix`, `docs`, `refactor`, `chore`, etc.).
- Extracting breaking change footers (`BREAKING CHANGE:`).
- Classifying semver bump severity (`major`, `minor`, `patch`) from commit history.

## License

Permissively licensed under `MIT OR Apache-2.0`.
