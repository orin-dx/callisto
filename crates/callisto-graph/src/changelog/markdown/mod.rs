//! The one place Markdown is scanned. Finding a changelog section, spotting a duplicate heading and reading markers
//! all ask the same questions of a line: is it fenced code, is it a heading, does it open a block. Answer them here, once, so the passes cannot disagree; `corpus` runs every pass over the same
//! documents to prove it.

mod fences;
mod headings;
mod markers;

#[cfg(test)]
mod corpus;

pub(crate) use fences::Fences;
pub(crate) use markers::{extract_markers, MarkerKind};
