//! Many `<rev>:<path>` blobs from one `git cat-file --batch`, so a read costs a line of stdin and not a process.

use callisto_model::CommandRunner;

use super::release_decision::{command_malformed_output, command_non_zero_exit};
use crate::GraphError;

/// One object's content as reported by `git cat-file --batch`, for a single
/// requested `<commit>:<path>`.
pub(crate) enum BatchBlob {
    /// The path resolved to a blob at the queried commit; this is its raw
    /// content.
    Blob(String),
    /// The path did not exist at the queried commit -- e.g. a package's
    /// manifest that was only added later, so it has no blob yet at an
    /// earlier `parent` commit.
    Missing,
}

/// Issues exactly one `git cat-file --batch` invocation requesting every
/// object in `objects` (each `<rev>:<path>`), and returns each result in the
/// same order as `objects`.
///
/// `git cat-file --batch` reads one object identifier per line from stdin
/// and, for each, writes to stdout either:
///
/// ```text
/// <sha> SP <type> SP <size> LF
/// <content: exactly `size` bytes> LF
/// ```
///
/// or, if the object doesn't exist:
///
/// ```text
/// <object> SP missing LF
/// ```
///
/// (git also reports `<object> SP ambiguous LF` for a ref that resolves to
/// more than one object; `<commit>:<path>` syntax should never be
/// ambiguous, but it's checked for explicitly below rather than silently
/// falling through to the blob-header parse.)
///
/// This lets every canonical manifest's content at one commit be fetched in
/// a single subprocess round trip instead of one `git show` per manifest.
///
/// Parsing here is deliberately defensive rather than permissive: this
/// feeds [`derive_release_commit_decision`]'s trust-boundary comparison, so
/// any framing ambiguity -- an unrecognized header shape, a declared `size`
/// that doesn't fit the remaining output, a missing separator byte right
/// after the declared content length, a non-`blob` object type, or a
/// leftover/truncated tail once every requested path has been accounted
/// for -- fails the whole batch closed rather than guessing. Each expected
/// response is also matched against its exact requested object string
/// (rather than a generic "ends with ` missing`" pattern), so a manifest
/// path that happened to contain a space or colon can't be misread as a
/// different response shape. Silently misattributing one manifest's
/// content to a different path because an undetected desync shifted every
/// later offset would be a far worse failure mode than an outright
/// rejection.
pub(crate) fn batch_blobs<R: CommandRunner + ?Sized>(
    runner: &R,
    root: &std::path::Path,
    objects: &[String],
) -> Result<Vec<BatchBlob>, GraphError> {
    if objects.is_empty() {
        return Ok(Vec::new());
    }

    const BATCH_ARGS: [&str; 2] = ["cat-file", "--batch"];

    let mut stdin = String::new();
    for object in objects {
        stdin.push_str(object);
        stdin.push('\n');
    }

    let output = runner.run_with_stdin("git", &BATCH_ARGS, root, stdin.as_bytes())?;
    if !output.success() {
        return Err(command_non_zero_exit("git", &BATCH_ARGS, &output));
    }

    let mut rest = output.stdout.as_str();
    let mut results = Vec::with_capacity(objects.len());
    for object in objects {
        let (header, after_header) = rest.split_once('\n').ok_or_else(|| {
            command_malformed_output(
                "git",
                &BATCH_ARGS,
                format!("truncated response: missing header line for `{object}`"),
            )
        })?;

        if header == format!("{object} missing") {
            results.push(BatchBlob::Missing);
            rest = after_header;
            continue;
        }
        if header == format!("{object} ambiguous") {
            return Err(command_malformed_output(
                "git",
                &BATCH_ARGS,
                format!("object `{object}` is ambiguous"),
            ));
        }

        let mut fields = header.split(' ');
        let sha = fields.next().ok_or_else(|| {
            command_malformed_output(
                "git",
                &BATCH_ARGS,
                format!("header for `{object}` is missing its sha field: `{header}`"),
            )
        })?;
        let object_type = fields.next().ok_or_else(|| {
            command_malformed_output(
                "git",
                &BATCH_ARGS,
                format!("header for `{object}` is missing its type field: `{header}`"),
            )
        })?;
        let size_field = fields.next().ok_or_else(|| {
            command_malformed_output(
                "git",
                &BATCH_ARGS,
                format!("header for `{object}` is missing its size field: `{header}`"),
            )
        })?;
        if fields.next().is_some() {
            // A well-formed header has exactly three space-separated
            // fields; anything else is not a shape this parser understands.
            return Err(command_malformed_output(
                "git",
                &BATCH_ARGS,
                format!("header for `{object}` has more than three fields: `{header}`"),
            ));
        }
        if sha.is_empty() || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(command_malformed_output(
                "git",
                &BATCH_ARGS,
                format!("header for `{object}` reports a non-hex sha: `{sha}`"),
            ));
        }
        if object_type != "blob" {
            // Every path requested here is a canonical manifest file, so a
            // resolved object must be a blob; a tree/commit/tag response
            // means the path resolved to something else entirely.
            return Err(command_malformed_output(
                "git",
                &BATCH_ARGS,
                format!("object `{object}` resolved to a `{object_type}`, not a blob"),
            ));
        }
        let size: usize = size_field.parse().map_err(|parse_error| {
            command_malformed_output(
                "git",
                &BATCH_ARGS,
                format!("header for `{object}` has a non-numeric size `{size_field}`: {parse_error}"),
            )
        })?;

        if size > after_header.len() {
            return Err(command_malformed_output(
                "git",
                &BATCH_ARGS,
                format!("object `{object}` declares size {size}, exceeding the remaining response"),
            ));
        }
        // `.get(..size)` (byte-indexed) rather than raw slicing: if `size`
        // doesn't land on a UTF-8 char boundary this fails closed instead
        // of panicking.
        let content = after_header.get(..size).ok_or_else(|| {
            command_malformed_output(
                "git",
                &BATCH_ARGS,
                format!("object `{object}` declares size {size} that does not land on a UTF-8 boundary"),
            )
        })?;
        let remainder = &after_header[size..];
        // The protocol always emits exactly one LF right after the
        // content, separate from the content itself. Requiring it here
        // catches a desynced `size` (from an earlier, unexpectedly
        // reencoded entry) as an explicit error instead of silently
        // parsing the wrong bytes as this entry's content.
        if remainder.as_bytes().first() != Some(&b'\n') {
            return Err(command_malformed_output(
                "git",
                &BATCH_ARGS,
                format!("object `{object}` content is not followed by the expected newline separator"),
            ));
        }
        results.push(BatchBlob::Blob(content.to_string()));
        rest = &remainder[1..];
    }

    if !rest.is_empty() {
        // More output than the requested objects account for -- either an
        // extra unexpected reply or a framing desync earlier in the
        // stream. Either way, fail closed rather than ignore it.
        return Err(command_malformed_output(
            "git",
            &BATCH_ARGS,
            "response contained more data than the requested objects account for",
        ));
    }

    Ok(results)
}
