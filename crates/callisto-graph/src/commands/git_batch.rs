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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use callisto_model::{CommandError, CommandOutput, CommandRunner};

    use super::*;

    /// A runner whose `git cat-file --batch` answers with a fixed output and exit code.
    struct Answers {
        stdout: String,
        exit_code: i32,
    }

    impl CommandRunner for Answers {
        fn run(&self, _: &str, _: &[&str], _: &Path) -> Result<CommandOutput, CommandError> {
            panic!("only the batch read is expected");
        }

        fn run_with_stdin(&self, _: &str, _: &[&str], _: &Path, _: &[u8]) -> Result<CommandOutput, CommandError> {
            Ok(CommandOutput {
                exit_code: Some(self.exit_code),
                stdout: self.stdout.clone(),
                stderr: String::new(),
            })
        }
    }

    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

    fn read(stdout: &str, objects: &[&str]) -> Result<Vec<BatchBlob>, GraphError> {
        let runner = Answers {
            stdout: stdout.to_owned(),
            exit_code: 0,
        };
        let objects: Vec<String> = objects.iter().map(|object| (*object).to_owned()).collect();
        batch_blobs(&runner, Path::new("."), &objects)
    }

    fn malformed(stdout: &str, objects: &[&str]) -> String {
        match read(stdout, objects) {
            Err(error) => error.to_string(),
            Ok(_) => panic!("`{stdout:?}` must be refused"),
        }
    }

    #[test]
    fn blobs_and_missing_paths_come_back_in_request_order() {
        let stdout = format!("{SHA} blob 5\nhello\nHEAD:gone missing\n{SHA} blob 0\n\n");

        let blobs = read(&stdout, &["HEAD:a", "HEAD:gone", "HEAD:empty"]).unwrap();

        assert!(matches!(&blobs[0], BatchBlob::Blob(content) if content == "hello"));
        assert!(matches!(blobs[1], BatchBlob::Missing));
        assert!(matches!(&blobs[2], BatchBlob::Blob(content) if content.is_empty()));
    }

    #[test]
    fn no_objects_ask_nothing() {
        struct Never;
        impl CommandRunner for Never {
            fn run(&self, _: &str, _: &[&str], _: &Path) -> Result<CommandOutput, CommandError> {
                panic!("no process for no objects");
            }
        }

        assert!(batch_blobs(&Never, Path::new("."), &[]).unwrap().is_empty());
    }

    #[test]
    fn a_failing_git_fails_the_read() {
        let runner = Answers {
            stdout: String::new(),
            exit_code: 128,
        };

        assert!(batch_blobs(&runner, Path::new("."), &["HEAD:a".to_owned()]).is_err());
    }

    /// Each way the framing can be wrong fails the whole batch, so one manifest's text is never taken for another's.
    #[test]
    fn every_framing_defect_fails_closed() {
        let cases: [(&str, String, &[&str]); 11] = [
            ("no header line", "HEAD:a".to_owned(), &["HEAD:a"]),
            (
                "a missing object that was not the one asked for",
                "HEAD:b missing\n".to_owned(),
                &["HEAD:a"],
            ),
            ("an ambiguous name", "HEAD:a ambiguous\n".to_owned(), &["HEAD:a"]),
            ("a header with too few fields", format!("{SHA} blob\nx\n"), &["HEAD:a"]),
            (
                "a header with too many fields",
                format!("{SHA} blob 1 extra\nx\n"),
                &["HEAD:a"],
            ),
            ("a sha that is not hex", "zzzz blob 1\nx\n".to_owned(), &["HEAD:a"]),
            (
                "a tree where a blob was asked for",
                format!("{SHA} tree 1\nx\n"),
                &["HEAD:a"],
            ),
            (
                "a size that is not a number",
                format!("{SHA} blob many\nx\n"),
                &["HEAD:a"],
            ),
            (
                "a size larger than the output",
                format!("{SHA} blob 99\nx\n"),
                &["HEAD:a"],
            ),
            ("no newline after the content", format!("{SHA} blob 1\nxy"), &["HEAD:a"]),
            (
                "more output than objects asked for",
                format!("{SHA} blob 1\nx\nextra\n"),
                &["HEAD:a"],
            ),
        ];
        for (what, stdout, objects) in cases {
            assert!(read(&stdout, objects).is_err(), "{what} must be refused");
        }
    }

    #[test]
    fn a_size_inside_a_character_is_refused_not_a_panic() {
        // "é" is two bytes, so a declared size of one lands inside it.
        let message = malformed(&format!("{SHA} blob 1\né\n"), &["HEAD:a"]);

        assert!(message.contains("UTF-8"), "{message}");
    }

    #[test]
    fn a_desynced_size_cannot_hand_one_paths_content_to_another() {
        // The first object declares one byte too few, so the second header is no longer where it should be.
        let stdout = format!("{SHA} blob 4\nabcde\n{SHA} blob 1\nz\n");

        assert!(read(&stdout, &["HEAD:a", "HEAD:b"]).is_err());
    }
}
