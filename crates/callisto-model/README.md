# callisto-model

Domain primitives, SemVer grammars, package identity types, and versioned JSON report contracts for Callisto.

## Overview

`callisto-model` defines core type primitives for Callisto versioning and release management workflows:

- Package identity representation (`PackageId`, `PackageName`, `Ecosystem`).
- SemVer version parsing, comparison, and bump severity classification (`Severity`).
- Diagnostic errors using `miette` and standard error implementations.
- Serde-compatible JSON schemas for machine-readable publish and version plans.

## License

Permissively licensed under `MIT OR Apache-2.0`.
