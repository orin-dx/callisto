# cargo info fixtures

Captured 2026-09-20 with cargo 1.96.0 (30a34c682 2026-05-25) by
`testing/refresh-provider-fixtures.sh`. Read-only queries, no credentials.

Each `.cmd` file is the exit code, stdout and stderr of one real run, in the
envelope `exit: N`, `--- stdout`, `--- stderr`. The unit tests feed these bytes
to the production classifier, so the observation is held to what cargo actually
prints rather than to a guess at it.

Every run is issued from a scratch package with an explicit `--registry`, which
is exactly what the release path does: without `--registry`, `cargo info`
resolves the local workspace and reports an unpublished member's on-disk
version as published.

- `found.cmd`: `cargo info serde@1.0.0 --registry crates-io`. Exit 0; stdout
  carries the `version: 1.0.0 (latest ...)` line the classifier matches. The
  `latest` version drifts with every serde release; only the shape is checked.
- `absent-version.cmd`: `cargo info serde@0.0.0-definitely-not --registry crates-io`.
- `unknown-crate.cmd`: `cargo info callisto-model-no-such-crate-zz@1.0.0 --registry crates-io`.
- `yanked.cmd`: `cargo info chacha20@0.10.1 --registry crates-io`. chacha20
  0.10.1 is yanked, and cargo reports it with the *same* "could not find" line
  as an absent version: `cargo info` cannot see yanks. The refresh script fails
  if that line stops appearing, which is the signal that this fixture — and the
  yanked-reads-absent rule it documents — needs revisiting.
- `network-failure.cmd`: `cargo info serde@1.0.0 --registry unreachable`, where
  `unreachable` is a `sparse+https://127.0.0.1:9/index/` registry defined in the
  probe package's own `.cargo/config.toml`. Exit 101 like the three above, but
  with a different message (`failed to load source for dependency`), which is
  why classification requires the exact "could not find" phrase before it may
  answer `Absent`.
