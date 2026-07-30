# callisto-format

Changeset markdown parser and writer for Callisto monorepo version management.

## Overview

`callisto-format` provides bidirectional serialization for human-readable markdown changeset files stored in `.changeset/*.md`:

- Parse frontmatter declaring package bump types (`major`, `minor`, `patch`).
- Parse and serialize release summaries and changelog entries.
- Format-preserving markdown document construction.

## License

Permissively licensed under `MIT OR Apache-2.0`.
