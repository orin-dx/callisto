//! Path-keyed, read-only cache of opened [`Manifest`] handles.
//!
//! A single package manifest (e.g. one `Cargo.toml`) is consulted from three
//! independent read-only sites during a normal command run:
//!
//! 1. [`crate::walk::ManifestWalkResolver::build`] — once to collect
//!    [`Manifest::publish_targets`], once more to collect
//!    [`Manifest::iter_dependencies`].
//! 2. [`crate::Workspace::base_versions`] — once to read
//!    [`Manifest::current_version`].
//!
//! Without memoization each of those call sites independently invokes
//! [`callisto_manifests::open`], which re-reads the file from disk and
//! re-parses it, even though nothing on disk can have changed between them
//! within a single read-only command run. [`open_cached`] gives those three
//! sites a shared, path-keyed cache of `Arc<dyn Manifest>` handles so the
//! same manifest is opened at most once per run.
//!
//! This cache is deliberately **read-only**. `Manifest::current_version`,
//! `Manifest::publish_targets`, and `Manifest::iter_dependencies` are all
//! `&self` methods, so sharing a handle behind an `Arc` is sound. The
//! mutation path (`Manifest::write_version` / `Manifest::update_dependency_spec`,
//! invoked from `apply.rs` while applying a version plan) takes `&mut self`
//! and therefore cannot be served from this cache — nor should it be: reusing
//! a cached handle across a write would risk applying an edit against a
//! stale in-memory copy while another cached reader still holds (and might
//! later re-read) the pre-edit state. `apply.rs` intentionally keeps calling
//! `callisto_manifests::open` directly for every write.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use callisto_manifests::{open, Manifest, OpenContext};
use callisto_model::{ManifestDecl, ManifestError};

/// Opens `decl`, reusing a previously opened handle for the same path from
/// `cache` if one exists. Only suitable for read-only access to the
/// returned handle.
pub(crate) fn open_cached(
    cache: &RefCell<BTreeMap<PathBuf, Arc<dyn Manifest>>>,
    decl: &ManifestDecl,
    ctx: &OpenContext<'_>,
) -> Result<Arc<dyn Manifest>, ManifestError> {
    if let Some(existing) = cache.borrow().get(&decl.path) {
        return Ok(Arc::clone(existing));
    }

    let opened: Arc<dyn Manifest> = Arc::from(open(decl, ctx)?);
    cache.borrow_mut().insert(decl.path.clone(), Arc::clone(&opened));
    Ok(opened)
}
