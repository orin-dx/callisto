//! The one place Markdown is scanned. Finding a changelog section, spotting a duplicate heading, rebasing an entry's
//! headings and splitting off its headline all ask the same questions of a line: is it fenced code, is it a heading,
//! does it open a block. Answer them here, once, so the passes cannot disagree; `corpus` runs every pass over the same
//! documents to prove it.

mod fences;
mod headings;
mod summary;

#[cfg(test)]
mod corpus;

pub(crate) use fences::Fences;
pub(crate) use headings::rebase_headings;
pub(crate) use summary::split_summary;
