//! The one contract for interactive prompts: where they may run, how they end, and how `$EDITOR` is used.

use std::io::IsTerminal as _;

use inquire::validator::Validation;
use inquire::Text;

use crate::cli::OutputFormat;
use crate::error::CliError;

/// Proof that this run may prompt; only [`Prompts::detect`] makes one.
pub struct Prompts(());

impl Prompts {
    /// Prompts need a person at the terminal (stdin, and stderr where they draw) and a text run: `--format json` is for
    /// scripts and never prompts.
    pub fn detect(format: OutputFormat) -> Option<Self> {
        (format == OutputFormat::Text && std::io::stdin().is_terminal() && std::io::stderr().is_terminal())
            .then_some(Self(()))
    }

    #[cfg(test)]
    pub fn for_tests() -> Self {
        Self(())
    }
}

/// Printed whenever a prompt run ends without writing.
pub const CANCELLED: &str = "Cancelled, nothing written.";

/// Exit status for a prompt cancelled with Esc or Ctrl-C: 130, as shells report SIGINT.
pub const CANCELLED_EXIT: u8 = 130;

/// A package in a list: the name the list shows, and its `ecosystem/name` id, which is also accepted when typed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Listed {
    pub name: String,
    pub id: String,
}

impl Listed {
    /// An entry shown by its id.
    pub fn by_id(id: &str) -> Self {
        Self {
            name: id.to_owned(),
            id: id.to_owned(),
        }
    }
}

/// Asks which of `items` to pick, as `git add -i` does: the list is printed once, numbered, with `marked` ones starred,
/// then one typed line answers. Nothing is redrawn, so no terminal size or resize can misdraw it. Enter keeps the marked
/// ones (and, when `empty_ok`, picks none if none is marked); an answer that names nothing listed is asked again with
/// the reason.
pub(crate) fn pick_many(
    heading: &str,
    items: &[Listed],
    marked: &[bool],
    empty_ok: bool,
) -> Result<Vec<usize>, CliError> {
    print_list(heading, items, marked);
    if marked.contains(&true) {
        eprintln!("Enter keeps the * ones, or type numbers, ranges (1-3) or names; Esc cancels.");
    } else if empty_ok {
        eprintln!("Enter picks none, or type numbers, ranges (1-3) or names; Esc cancels.");
    } else {
        eprintln!("Type numbers, ranges (1-3) or names; Esc cancels.");
    }
    read_answer(items, marked, false)
}

/// Asks for exactly one of `ids`, the way [`pick_many`] asks for several.
pub(crate) fn pick_one(heading: &str, ids: &[String]) -> Result<usize, CliError> {
    let items: Vec<Listed> = ids.iter().map(|id| Listed::by_id(id)).collect();
    print_list(heading, &items, &[]);
    eprintln!("Type a number or a name; Esc cancels.");
    let picked = read_answer(&items, &[], true)?;
    Ok(*picked
        .first()
        .expect("the validator accepts only an answer naming exactly one entry"))
}

fn print_list(heading: &str, items: &[Listed], marked: &[bool]) {
    let width = items.len().to_string().len();
    let mut text = format!("{heading}\n");
    for (index, item) in items.iter().enumerate() {
        let mark = if marked.get(index) == Some(&true) { '*' } else { ' ' };
        // A control character in a manifest's name must not drive the terminal.
        text.push_str(&format!(
            "  {:>width$} {mark} {}\n",
            index + 1,
            item.name.escape_debug()
        ));
    }
    eprint!("{text}");
}

/// One line typed at a single-line prompt; the prompt's validator asks again with the reason until [`answer`] accepts
/// it, and Esc or Ctrl-C is [`CliError::Cancelled`].
fn read_answer(items: &[Listed], marked: &[bool], one: bool) -> Result<Vec<usize>, CliError> {
    let (valid_items, valid_marked) = (items.to_vec(), marked.to_vec());
    let line = Text::new("Your answer")
        .with_validator(move |line: &str| {
            Ok(match answer(line, &valid_items, &valid_marked, one) {
                Ok(_) => Validation::Valid,
                Err(reason) => Validation::Invalid(reason.into()),
            })
        })
        .prompt()?;
    Ok(answer(&line, items, marked, one).expect("the validator accepted this line"))
}

/// The listed indices a typed line picks: the `marked` ones for an empty line (when several may be picked), else the
/// ones [`choices`] names; `one` requires exactly one.
fn answer(line: &str, items: &[Listed], marked: &[bool], one: bool) -> Result<Vec<usize>, String> {
    let picked = if line.trim().is_empty() && !one {
        (0..items.len())
            .filter(|&index| marked.get(index) == Some(&true))
            .collect()
    } else {
        choices(line, items)?
    };
    if one && picked.len() != 1 {
        return Err("pick exactly one".to_owned());
    }
    Ok(picked)
}

/// The listed indices an answer names, in list order. A word is a number or range counted from 1 (`3`, `1-3`) when
/// those rows exist, else a listed name or `ecosystem/name` id; words are separated by spaces or commas.
fn choices(answer: &str, items: &[Listed]) -> Result<Vec<usize>, String> {
    let mut picked = std::collections::BTreeSet::new();
    for word in answer
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|word| !word.is_empty())
    {
        let shown = word.escape_debug();
        let by_name = items.iter().position(|item| item.name == word || item.id == word);
        let indices = match (by_name, numbered(word, items.len())) {
            (Some(index), None | Some(Err(_))) => index..=index,
            (Some(index), Some(Ok(range))) if range == (index..=index) => range,
            (Some(index), Some(Ok(range))) => {
                let rows = range
                    .map(|row| format!("{0}-{0}", row + 1))
                    .collect::<Vec<_>>()
                    .join(",");
                let package = index + 1;
                let id = &items[index].id;
                return Err(if id == word {
                    format!(
                        "`{shown}` is both a package name and a row number; type `{package}-{package}` for the package or `{rows}` for the row"
                    )
                } else {
                    format!(
                        "`{shown}` is both a package name and a row number; type `{}` for the package or `{rows}` for the row",
                        id.escape_debug()
                    )
                });
            }
            (None, Some(range)) => range?,
            (None, None) => return Err(format!("`{shown}` is not a number, a range or a listed name")),
        };
        picked.extend(indices);
    }
    Ok(picked.into_iter().collect())
}

/// `n` or `a-b`, counted from 1, as indices; `None` when the word is shaped like neither, `Some(Err)` when the rows are
/// not in the list.
fn numbered(word: &str, len: usize) -> Option<Result<std::ops::RangeInclusive<usize>, String>> {
    let (first, last) = word.split_once('-').unwrap_or((word, word));
    let digits = |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    if !digits(first) || !digits(last) {
        return None;
    }
    let listed = |text: &str| text.parse::<usize>().ok().filter(|number| (1..=len).contains(number));
    Some(match (listed(first), listed(last)) {
        (Some(low), Some(high)) if low <= high => Ok(low - 1..=high - 1),
        (Some(_), Some(_)) => Err(format!("`{word}` runs backwards; put the lower number first")),
        _ => Err(format!("`{word}` is not in the list (1-{len})")),
    })
}

/// The editor named by `$VISUAL`, else `$EDITOR`.
pub struct Editor {
    variable: &'static str,
    command: String,
}

/// Why the editor could not be used; its message is the warning to print.
#[derive(Debug, thiserror::Error)]
#[error("cannot use ${variable} (`{command}`): {reason}; asking for the summary here instead")]
pub struct EditorUnusable {
    variable: &'static str,
    command: String,
    reason: String,
}

impl Editor {
    /// An empty or blank setting is unset.
    pub fn from_env() -> Option<Self> {
        ["VISUAL", "EDITOR"].into_iter().find_map(|variable| {
            let command = std::env::var(variable).ok()?;
            (!command.trim().is_empty()).then_some(Self { variable, command })
        })
    }

    /// `VISUAL` or `EDITOR`, whichever named it.
    pub fn variable(&self) -> &'static str {
        self.variable
    }

    /// Opens the editor on an empty file and returns what was saved. `None` when the editor exits non-zero (an abort,
    /// as `:cq` in Vim). The editor draws on stderr so stdout stays for the report.
    pub fn edit(self) -> Result<Option<String>, EditorUnusable> {
        self.try_edit().map_err(|reason| EditorUnusable {
            variable: self.variable,
            command: self.command,
            reason,
        })
    }

    fn try_edit(&self) -> Result<Option<String>, String> {
        let words =
            shell_words::split(&self.command).map_err(|error| format!("it does not split into words: {error}"))?;
        let Some((program, args)) = words.split_first() else {
            return Err("it names no program".to_owned());
        };
        let failed = |error: std::io::Error| error.to_string();
        let file = tempfile::Builder::new()
            .prefix("callisto-")
            .suffix(".txt")
            .tempfile()
            .map_err(failed)?;
        let _editor_owns_ctrl_c = editor_owns_ctrl_c::enter().map_err(failed)?;
        let status = crate::runner::child_command(program, &[], &[])
            .args(args)
            .arg(file.path())
            .stdout(std::io::stderr())
            .status()
            .map_err(|error| format!("`{program}` did not start: {error}"))?;
        if !status.success() {
            return Ok(None);
        }
        let saved = std::fs::read(file.path()).map_err(failed)?;
        String::from_utf8(saved)
            .map(Some)
            .map_err(|error| format!("what it saved is not UTF-8 text: {}", error.utf8_error()))
    }
}

#[cfg(unix)]
mod editor_owns_ctrl_c {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use signal_hook::consts::SIGINT;

    /// Ctrl-C in a line-mode editor reaches this process too: as Git's `launch_editor` does, ignore SIGINT while the
    /// editor runs and let its exit status decide. Dropping this gives SIGINT its default action back.
    pub struct Guard(Arc<AtomicBool>);

    pub fn enter() -> std::io::Result<Guard> {
        let default_action = Arc::new(AtomicBool::new(false));
        signal_hook::flag::register_conditional_default(SIGINT, Arc::clone(&default_action))?;
        Ok(Guard(default_action))
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
}

#[cfg(not(unix))]
mod editor_owns_ctrl_c {
    pub struct Guard;

    pub fn enter() -> std::io::Result<Guard> {
        Ok(Guard)
    }
}

#[cfg(test)]
mod tests {
    use super::{answer, choices, Listed};

    fn names(names: &[&str]) -> Vec<Listed> {
        names.iter().map(|name| Listed::by_id(name)).collect()
    }

    /// npm packages shown by their bare names.
    fn npm(names: &[&str]) -> Vec<Listed> {
        names
            .iter()
            .map(|name| Listed {
                name: (*name).to_owned(),
                id: format!("npm/{name}"),
            })
            .collect()
    }

    #[test]
    fn an_answer_picks_by_number_range_and_name_in_list_order() {
        let listed = names(&["a", "b", "c", "d", "cargo/e"]);
        assert_eq!(choices("3, 1", &listed), Ok(vec![0, 2]));
        assert_eq!(choices("2-4", &listed), Ok(vec![1, 2, 3]));
        assert_eq!(choices("cargo/e a 1", &listed), Ok(vec![0, 4]));
        assert_eq!(choices("1-1,,  b", &listed), Ok(vec![0, 1]));
    }

    #[test]
    fn an_answer_naming_nothing_listed_says_why() {
        let listed = names(&["a", "b", "c"]);
        for (answer, reason) in [
            ("0", "`0` is not in the list (1-3)"),
            ("4", "`4` is not in the list (1-3)"),
            ("2-9", "`2-9` is not in the list (1-3)"),
            ("99999999999999999999999", "is not in the list (1-3)"),
            ("3-1", "`3-1` runs backwards"),
            ("x", "`x` is not a number, a range or a listed name"),
            ("1-", "`1-` is not a number"),
            ("-1", "`-1` is not a number"),
            ("A", "`A` is not a number"),
        ] {
            let error = choices(answer, &listed).expect_err(answer);
            assert!(error.contains(reason), "{answer}: {error}");
        }
    }

    #[test]
    fn a_name_that_reads_as_another_entrys_number_is_refused() {
        let ambiguous = "is both a package name and a row number";
        let refused = choices("2", &names(&["2", "b"])).unwrap_err();
        assert!(refused.contains(ambiguous));
        // Without a distinct qualified id the way out is the package's own row, never the word just typed.
        assert!(
            refused.contains("type `1-1` for the package or `2-2` for the row"),
            "{refused}"
        );
        assert!(choices("1-2", &names(&["a", "b", "1-2"]))
            .unwrap_err()
            .contains(ambiguous));
        // A name that is its own number is not ambiguous.
        assert_eq!(choices("1", &names(&["1", "b"])), Ok(vec![0]));
    }

    #[test]
    fn the_reason_for_an_ambiguous_word_says_how_to_pick_each() {
        // Rows: `2` then `1`; typing `1` could mean row 1 (package `2`) or the package named `1`.
        let listed = npm(&["2", "1"]);
        assert_eq!(
            choices("1", &listed).unwrap_err(),
            "`1` is both a package name and a row number; type `npm/1` for the package or `1-1` for the row"
        );
        assert_eq!(choices("npm/1", &listed), Ok(vec![1]));
        assert_eq!(choices("1-1", &listed), Ok(vec![0]));
        assert_eq!(choices("2-2", &listed), Ok(vec![1]));
        // A range collision lists the rows singly.
        let error = choices("1-2", &npm(&["a", "b", "1-2"])).unwrap_err();
        assert!(
            error.ends_with("type `npm/1-2` for the package or `1-1,2-2` for the row"),
            "{error}"
        );
    }

    #[test]
    fn a_qualified_id_is_accepted_as_a_listed_name() {
        let listed = npm(&["a", "b"]);
        assert_eq!(choices("npm/b a", &listed), Ok(vec![0, 1]));
        assert!(choices("cargo/a", &listed).is_err());
    }

    #[test]
    fn a_number_or_range_without_those_rows_is_matched_as_a_name() {
        assert_eq!(choices("42", &names(&["42"])), Ok(vec![0]));
        assert_eq!(choices("42", &names(&["a", "42"])), Ok(vec![1]));
        assert_eq!(choices("2-9", &names(&["a", "2-9"])), Ok(vec![1]));
        assert_eq!(choices("3-1", &names(&["a", "3-1"])), Ok(vec![1]));
        // The rows exist, so the number is a row.
        assert_eq!(choices("1", &names(&["42"])), Ok(vec![0]));
    }

    #[test]
    fn an_empty_line_keeps_the_marked_ones_and_one_pick_needs_exactly_one() {
        let listed = names(&["a", "b", "c"]);
        assert_eq!(answer("  ", &listed, &[false, true, true], false), Ok(vec![1, 2]));
        assert_eq!(answer("", &listed, &[], false), Ok(vec![]));
        assert_eq!(answer("2", &listed, &[], true), Ok(vec![1]));
        for line in ["", "1 2", "1-2"] {
            assert_eq!(
                answer(line, &listed, &[], true),
                Err("pick exactly one".to_owned()),
                "{line:?}"
            );
        }
    }
}
