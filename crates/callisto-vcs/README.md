# callisto-vcs

Git access for Callisto, through the system `git` binary.

## Overview

`callisto-vcs` runs every Git read and write as a `git` subprocess through a `CommandRunner`:

- Commit history since a tag, scoped to package paths.
- Tag listing, ref resolution, and tag creation.
- Staged-change and release-trust observation.

## License

MIT License (`MIT`).
