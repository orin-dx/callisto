//! Track 0 spike: does gix low-level plumbing read a real git repo from inside
//! a WASM guest via WASI filesystem preopens?
//!
//! ## Result (run 2026-08-04, wasmtime 47.0.3)
//!
//! **PARTIAL: ref reads work; object reads blocked by mmap.**
//!
//! ```text
//! .git exists: true
//! gix-odb opened OK
//! gix-ref opened OK
//! HEAD: f0672c71c05eca4c47d129f3a2abb892b8fe64bc
//! FAIL: Find(Loose(Io { source: Kind(Unsupported), action: "open or map", path: "/.git/objects/f0/..." }))
//! ```
//!
//! gix-odb attempts to memory-map loose object files.  WASI preopens support
//! regular file I/O (open, read, seek) but NOT mmap; mmap returns ENOSYS
//! (`std::io::ErrorKind::Unsupported`).  Pack-file reading would hit the same
//! blocker since gix-pack also uses mmap for index and data files.
//!
//! gix-ref reads (HEAD resolution, branch lookup) work correctly: they use
//! regular file reads and are not affected by the mmap restriction.
//!
//! ## Compilation prerequisites
//!
//! gix-hash requires the `sha1` feature to be non-empty when targeting WASM
//! (without any hash feature enabled, the `Kind` enum has no variants and all
//! match arms are non-exhaustive — a compile error).  The `gix-spike` feature
//! flag in callisto-moon/Cargo.toml gates the required `sha1` feature on
//! gix-odb and gix-hash.
//!
//! Additionally, the `http-cache-reqwest` dev-dependency in callisto-moon
//! (pinned to work around a warpgate transitive resolution issue) pulls in
//! reqwest → hyper-rustls → rustls → aws-lc-rs → aws-lc-sys (C library),
//! which cannot compile for wasm32-wasip1.  The spike must be compiled in a
//! standalone crate (not as a callisto-moon example) to avoid this.
//!
//! ## Standalone compile and run
//!
//! ```sh
//! # From /tmp/gix-wasm-probe (Cargo.toml: gix-odb/gix-ref/gix-hash with sha1)
//! cargo build --target wasm32-wasip1
//! wasmtime run --dir . -- target/wasm32-wasip1/debug/gix-wasm-probe.wasm
//! ```
//!
//! ## Track 0 verdict: NO-GO for gix-native object reads
//!
//! The mmap blocker rules out using gix-odb/gix-pack for commit-log reading
//! inside the WASM guest.  exec-seam via `warpgate_pdk::exec_command` (the
//! current callisto-moon design) is the correct approach: it shells out to the
//! host's git binary, which has no WASI restrictions.
//!
//! gix-ref IS usable for lightweight operations (read HEAD, resolve a branch
//! name) if callisto-moon ever needs those without a full commit walk.  A
//! hybrid (native ref reads + host-exec for commit data) is possible but adds
//! complexity for minimal gain; the exec-seam alone is simpler and sufficient.

#[cfg(not(feature = "gix-spike"))]
fn main() {
    eprintln!(
        "Recompile with --features gix-spike to run this spike.\n\
         See the file-level doc comment for the compile+run recipe."
    );
}

#[cfg(feature = "gix-spike")]
fn main() {
    if let Err(e) = run() {
        eprintln!("SPIKE FAIL: {e}");
        std::process::exit(1);
    }
}

#[cfg(feature = "gix-spike")]
fn run() -> Result<(), Box<dyn std::error::Error>> {
    use gix_odb::HeaderExt as _;

    let cwd = std::env::current_dir()?;
    println!("cwd: {}", cwd.display());

    let git_dir = cwd.join(".git");
    println!(".git exists: {}", git_dir.exists());

    let objects_dir = git_dir.join("objects");
    let odb = gix_odb::at(&objects_dir)?;
    println!("gix-odb opened OK");

    let ref_store = gix_ref::file::Store::at(
        git_dir.clone(),
        gix_ref::store::init::Options {
            object_hash: gix_hash::Kind::Sha1,
            ..Default::default()
        },
    );
    println!("gix-ref opened OK");

    let head_ref = ref_store.find("HEAD")?;
    let head_target = head_ref.target;
    let head_id = match head_target {
        gix_ref::Target::Object(id) => id,
        gix_ref::Target::Symbolic(name) => {
            let resolved = ref_store.find(name.as_bstr())?;
            resolved
                .target
                .try_id()
                .ok_or("symbolic ref did not resolve to an oid")?
                .to_owned()
        }
    };
    println!("HEAD: {head_id}");

    // This call succeeds for packed objects via file I/O but fails for loose
    // objects because gix-odb attempts to mmap the loose object file and WASI
    // returns ENOSYS (Unsupported) for mmap.
    let header = odb.header(head_id.as_ref())?;
    println!("HEAD object: kind={:?} size={}", header.kind(), header.size());

    println!("SPIKE SUCCESS");
    Ok(())
}
