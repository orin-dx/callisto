exit: 0
--- stdout
serde #serde #serialization #no_std
A generic serialization/deserialization framework
version: 1.0.0 (latest 1.0.229)
license: MIT/Apache-2.0
rust-version: unknown
documentation: https://docs.serde.rs/serde/
homepage: https://serde.rs
repository: https://github.com/serde-rs/serde
crates.io: https://crates.io/crates/serde/1.0.0
features:
 +default      = [std]
  std          = []
  alloc        = [unstable]
  collections  = [alloc]
  derive       = [serde_derive]
  playground   = [serde_derive]
  rc           = []
  serde_derive = [dep:serde_derive]
  unstable     = []
--- stderr
    Updating crates.io index
