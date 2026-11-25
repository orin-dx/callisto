use std::fs;
use std::process::ExitCode;

use callisto_graph::DependencyResolver;
use callisto_model::format::{Changeset, Entry};
use callisto_model::{AddReport, ApplyPermit, Severity, SCHEMA_VERSION};
use inquire::validator::Validation;
use inquire::{Confirm, Text};

use crate::cli::{AddArgs, GlobalArgs, OutputFormat};
use crate::error::CliError;
use crate::output::{emit_line, emit_report, log_line, prompt_line};
use crate::runner::CliCommandRunner;
use crate::tty::{self, Listed};
use crate::workspace::load_workspace;

pub fn handle(args: AddArgs, global: &GlobalArgs) -> Result<ExitCode, CliError> {
    let runner = CliCommandRunner;
    let ws = load_workspace(global, &runner)?;

    let mut entries = Vec::new();
    // A run that cannot prompt says so before any flag is checked.
    let prompts = args
        .packages
        .is_empty()
        .then(|| {
            tty::Prompts::detect(global.format).ok_or_else(|| {
                let mut missing = vec!["--package"];
                missing.extend(args.summary.is_none().then_some("--summary"));
                CliError::NotATty { missing }
            })
        })
        .transpose()?;
    // Built before any prompt, so a blank flag fails before the review.
    let mut summary = args.summary.as_deref().map(Summary::new).transpose()?;
    let mut wizard = false;

    if let Some(prompts) = prompts {
        // The same derivation `status` reports as `changed_since_last_tag`.
        let packages: Vec<&callisto_model::Package> = ws.graph.packages().collect();
        let changed = ws
            .tags()
            .and_then(|tags| callisto_graph::changed::changed_since_last_tag(&packages, tags, ws.git_access()));
        let choices = wizard_choices(packages.iter().copied(), changed);
        let mut prompter = TerminalAddPrompter { _allowed: prompts };
        let Some((chosen, written)) = run_wizard(&choices, summary.clone(), global.dry_run, &mut prompter)? else {
            return Ok(ExitCode::SUCCESS);
        };
        entries = chosen;
        summary = Some(written);
        wizard = true;
    } else {
        // Non-interactive mode (flags supplied via CLI or agent)
        for pkg_str in args.packages {
            let (name, sev_str) = pkg_str
                .rsplit_once(':')
                .ok_or_else(|| CliError::AddInvalidPackageSpec { spec: pkg_str.clone() })?;

            let severity = parse_severity(sev_str)?;

            // Resolves by any registered native name (not just a package's
            // primary/display name), reporting E102/E103 on a miss or
            // ecosystem-ambiguous bare name.
            callisto_graph::ConfigError::require_qualified(name, "--package")
                .map_err(callisto_graph::GraphError::Config)?;
            let resolved = ws.identity.resolve(name)?;

            let package = resolved.display_name();
            if entries.iter().any(|e: &Entry| e.name == package) {
                return Err(CliError::AddDuplicatePackage {
                    package: name.to_owned(),
                });
            }

            entries.push(Entry {
                // Use display_name() so "cargo/foo" is preserved, not just "foo".
                // In a polyglot workspace, the bare name is ambiguous across ecosystems.
                name: package,
                severity,
            });
        }
    }

    let summary = summary.ok_or(CliError::AddSummaryRequired)?;
    let changeset = Changeset {
        entries,
        summary: summary.0,
    };
    let text = callisto_model::format::write_changeset(&changeset)?;

    let changeset_dir = ws.root.join(&ws.config.changesets_dir);
    let archive_dir = ws.root.join(ws.config.pre_archive_dir());

    let Some(permit) = ApplyPermit::granted_unless_dry_run(global.dry_run) else {
        // Compute what WOULD be written, but never touch disk.
        let filename = unused_changeset_filename(&changeset_dir, &archive_dir)?;
        let rel_path = ws.config.changesets_dir.join(&filename).to_string_lossy().into_owned();
        match global.format {
            OutputFormat::Json => {
                let report = AddReport {
                    schema_version: SCHEMA_VERSION,
                    path: rel_path,
                    content: Some(text),
                    diagnostics: vec![],
                };
                emit_report(&mut std::io::stdout(), &report, true)?;
            }
            OutputFormat::Text => {
                emit_line(&format!(
                    "[DRY-RUN] Would add changeset: {rel_path} (no files written)\n\n{text}"
                ))?;
            }
        }
        return Ok(ExitCode::SUCCESS);
    };

    fs::create_dir_all(&changeset_dir)?;
    let filename = create_changeset(&changeset_dir, &archive_dir, &text, &permit, generate_human_slug)?;
    let rel_path = ws.config.changesets_dir.join(&filename).to_string_lossy().into_owned();

    match global.format {
        OutputFormat::Json => {
            let report = AddReport {
                schema_version: SCHEMA_VERSION,
                path: rel_path,
                content: None,
                diagnostics: vec![],
            };
            emit_report(&mut std::io::stdout(), &report, false)?;
        }
        OutputFormat::Text => {
            log_line(global.format, &format!("Added changeset: {rel_path}"))?;
        }
    }
    if wizard {
        prompt_line("Next: commit it with your change; `callisto release --dry-run` previews the release.");
    }

    Ok(ExitCode::SUCCESS)
}

/// A changeset summary: trimmed, with LF line ends, and never blank, whichever way it was supplied.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Summary(String);

impl Summary {
    /// A leading byte-order mark (some editors save one) is dropped and CRLF line ends become LF.
    fn new(text: &str) -> Result<Self, CliError> {
        let trimmed = text.trim_start_matches('\u{feff}').trim();
        if trimmed.is_empty() {
            return Err(CliError::AddSummaryEmpty);
        }
        Ok(Self(trimmed.replace("\r\n", "\n")))
    }
}

/// The wizard's questions; a scripted implementation drives tests. Esc or Ctrl-C at any of them is
/// [`CliError::Cancelled`].
trait AddPrompter {
    /// The indices of the packages the contributor picked; `marked` ones are picked when they just press Enter.
    fn pick(
        &mut self,
        heading: &str,
        items: &[Listed],
        marked: &[bool],
        empty_ok: bool,
    ) -> Result<Vec<usize>, CliError>;
    /// The summary; an empty answer opens `$EDITOR` for a longer one. `None` when the editor saves nothing.
    fn summary(&mut self, prompt: &str) -> Result<Option<Summary>, CliError>;
    fn confirm(&mut self, prompt: &str, default: bool) -> Result<bool, CliError>;
}

struct TerminalAddPrompter {
    _allowed: tty::Prompts,
}

impl AddPrompter for TerminalAddPrompter {
    fn pick(
        &mut self,
        heading: &str,
        items: &[Listed],
        marked: &[bool],
        empty_ok: bool,
    ) -> Result<Vec<usize>, CliError> {
        tty::pick_many(heading, items, marked, empty_ok)
    }

    fn summary(&mut self, prompt: &str) -> Result<Option<Summary>, CliError> {
        let mut editor = tty::Editor::from_env();
        loop {
            let hint = match &editor {
                Some(editor) => format!("{prompt} (leave empty to open ${}; Esc cancels)", editor.variable()),
                None => format!("{prompt} (Esc cancels)"),
            };
            let opens_editor = editor.is_some();
            let line = Text::new(&hint)
                .with_validator(move |line: &str| {
                    Ok(match Summary::new(line) {
                        Err(_) if !(opens_editor && line.is_empty()) => {
                            Validation::Invalid("the summary cannot be blank".into())
                        }
                        _ => Validation::Valid,
                    })
                })
                .prompt()?;
            if let Ok(summary) = Summary::new(&line) {
                return Ok(Some(summary));
            }
            // The validator lets an empty line through only when there is an editor to open.
            let Some(chosen) = editor.take() else { continue };
            match chosen.edit() {
                Ok(saved) => return Ok(saved.and_then(|text| Summary::new(&text).ok())),
                // The answers so far are kept: the summary is asked for inline instead.
                Err(unusable) => prompt_line(&format!("warning: {unusable}")),
            }
        }
    }

    fn confirm(&mut self, prompt: &str, default: bool) -> Result<bool, CliError> {
        Ok(Confirm::new(prompt).with_default(default).prompt()?)
    }
}

/// A package the wizard offers, and whether it changed since its last tag.
struct Choice {
    item: Listed,
    changed: bool,
}

/// The interactive wizard (as in `@changesets/cli`): packages, which are major, which of the rest are minor (the
/// remainder default to patch), a summary unless one was given, then a review to confirm (a dry run writes nothing, so
/// it does not ask). Changed packages are listed
/// first and marked, so Enter picks them. `None` when the contributor declines the review or the editor saves nothing, having been
/// told nothing was written.
fn run_wizard(
    choices: &[Choice],
    summary: Option<Summary>,
    dry_run: bool,
    prompter: &mut impl AddPrompter,
) -> Result<Option<(Vec<Entry>, Summary)>, CliError> {
    let answers = ask(choices, summary, dry_run, prompter)?;
    if answers.is_none() {
        prompt_line(tty::CANCELLED);
    }
    Ok(answers)
}

fn ask(
    choices: &[Choice],
    summary: Option<Summary>,
    dry_run: bool,
    prompter: &mut impl AddPrompter,
) -> Result<Option<(Vec<Entry>, Summary)>, CliError> {
    if choices.is_empty() {
        return Err(CliError::AddNoPackagesInWorkspace);
    }

    let mut ordered: Vec<&Choice> = choices.iter().collect();
    ordered.sort_by_key(|choice| !choice.changed);
    let items: Vec<Listed> = ordered.iter().map(|choice| choice.item.clone()).collect();
    let changed: Vec<bool> = ordered.iter().map(|choice| choice.changed).collect();
    let selected_indices = prompter.pick(
        "Packages in this change (* changed since its last release):",
        &items,
        &changed,
        false,
    )?;
    if selected_indices.is_empty() {
        return Err(CliError::AddNoPackagesSelected);
    }
    let selected_packages: Vec<Listed> = selected_indices.into_iter().map(|i| items[i].clone()).collect();
    let unmarked = vec![false; selected_packages.len()];

    let majors = prompter.pick(
        "Which are a MAJOR bump? (breaking changes only)",
        &selected_packages,
        &unmarked,
        true,
    )?;
    let major_set: std::collections::HashSet<usize> = majors.into_iter().collect();

    let minor_candidates: Vec<Listed> = selected_packages
        .iter()
        .enumerate()
        .filter(|(idx, _)| !major_set.contains(idx))
        .map(|(_, item)| item.clone())
        .collect();
    let minor_set: std::collections::HashSet<String> = if minor_candidates.is_empty() {
        std::collections::HashSet::new()
    } else {
        let minors = prompter.pick(
            "Which are a MINOR bump? (the rest get PATCH)",
            &minor_candidates,
            &unmarked[..minor_candidates.len()],
            true,
        )?;
        minors
            .into_iter()
            .map(|idx| minor_candidates[idx].name.clone())
            .collect()
    };

    let entries: Vec<Entry> = selected_packages
        .iter()
        .enumerate()
        .map(|(idx, item)| Entry {
            name: item.name.clone(),
            severity: if major_set.contains(&idx) {
                Severity::Major
            } else if minor_set.contains(&item.name) {
                Severity::Minor
            } else {
                Severity::Patch
            },
        })
        .collect();

    let summary = match summary {
        Some(summary) => summary,
        None => match prompter.summary("Changelog summary")? {
            Some(summary) => summary,
            None => return Ok(None),
        },
    };

    prompt_line(&review(&entries, &summary.0));
    if dry_run || prompter.confirm("Write this changeset?", true)? {
        Ok(Some((entries, summary)))
    } else {
        Ok(None)
    }
}

/// What the changeset will record: each package's bump, then the summary as it reaches the changelog.
fn review(entries: &[Entry], summary: &str) -> String {
    let mut text = String::from("\nThis changeset records:\n");
    for entry in entries {
        // A control character in a manifest's name must not drive the terminal.
        text.push_str(&format!(
            "  {:<5}  {}\n",
            entry.severity.to_string(),
            entry.name.escape_debug()
        ));
    }
    text.push('\n');
    for line in summary.trim().lines() {
        text.push_str(&format!("  {line}\n"));
    }
    text
}

/// Parses a CLI-supplied severity string, reporting every value `Severity`
/// actually accepts (`none`, `patch`, `minor`, `major`) on failure -- not a
/// stale subset that omits `none`.
fn parse_severity(sev_str: &str) -> Result<Severity, CliError> {
    sev_str.parse().map_err(|_err| CliError::InvalidSeverity {
        value: sev_str.to_string(),
    })
}

/// The wizard's packages, named by `.display_name()` so `cargo/foo` and `npm/foo` stay distinct in a polyglot workspace.
/// When the changed packages cannot be worked out, says so once and offers every package unpicked.
fn wizard_choices<'a>(
    packages: impl Iterator<Item = &'a callisto_model::Package>,
    changed: Result<std::collections::BTreeMap<callisto_model::PackageId, bool>, callisto_graph::GraphError>,
) -> Vec<Choice> {
    let changed = changed.unwrap_or_else(|error| {
        prompt_line(&format!(
            "warning: could not tell which packages changed since their last release, so none is picked: {error}"
        ));
        std::collections::BTreeMap::new()
    });
    packages
        .map(|pkg| Choice {
            item: Listed {
                name: pkg.id.display_name(),
                id: pkg.canonical_id().display_name(),
            },
            changed: changed.get(&pkg.id).copied().unwrap_or(false),
        })
        .collect()
}

/// Writes `text` to a new file in `dir` named by `next_slug`, trying another slug whenever the name is taken, including
/// by a changeset another process created after this one looked. Nothing already in `dir` is ever replaced.
fn create_changeset(
    dir: &std::path::Path,
    archive: &std::path::Path,
    text: &str,
    permit: &ApplyPermit,
    mut next_slug: impl FnMut() -> String,
) -> Result<String, CliError> {
    // 8,000 possible slugs: more than a few attempts only happens when nearly all of them are taken.
    for _ in 0..64 {
        let filename = format!("{}.md", next_slug());
        if archived(archive, &filename)? {
            continue;
        }
        let path = dir.join(&filename);
        match callisto_model::atomic::atomic_write_new(&path, text, permit) {
            Ok(()) => return Ok(filename),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(source) => {
                return Err(CliError::Io {
                    source,
                    path: Some(path),
                })
            }
        }
    }
    Err(CliError::Io {
        source: std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "no unused changeset filename found after 64 attempts",
        ),
        path: Some(dir.to_path_buf()),
    })
}

/// Whether a Changesets v3 pre cycle archived a note under `filename`; a new note there would read as its duplicate.
fn archived(archive: &std::path::Path, filename: &str) -> Result<bool, CliError> {
    let path = archive.join(filename);
    callisto_model::strict_fs::exists(&path).map_err(|source| CliError::Io {
        source,
        path: Some(path),
    })
}

/// Picks a changeset filename that does not exist yet, so a slug collision never overwrites a pending changeset.
fn unused_changeset_filename(dir: &std::path::Path, archive: &std::path::Path) -> Result<String, CliError> {
    // 8,000 possible slugs: more than a few attempts only happens when nearly all of them are taken.
    for _ in 0..64 {
        let filename = format!("{}.md", generate_human_slug());
        let path = dir.join(&filename);
        let taken = callisto_model::strict_fs::exists(&path).map_err(|source| CliError::Io {
            source,
            path: Some(path.clone()),
        })?;
        if !taken && !archived(archive, &filename)? {
            return Ok(filename);
        }
    }
    Err(CliError::Io {
        source: std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "no unused changeset filename found after 64 attempts",
        ),
        path: Some(dir.to_path_buf()),
    })
}

fn generate_human_slug() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let adjectives = [
        "swift", "clever", "bright", "silent", "brave", "calm", "eager", "gentle", "happy", "jolly", "keen", "lively",
        "mighty", "noble", "proud", "quick", "radiant", "sharp", "tough", "vivid",
    ];
    let nouns = [
        "foxes", "hawks", "wolves", "eagles", "bears", "lions", "otters", "pandas", "falcons", "tigers", "dolphins",
        "panthers", "cheetahs", "leopards", "ravens", "badgers", "lynxes", "cobras", "owls", "stags",
    ];
    let verbs = [
        "run", "fly", "leap", "soar", "dash", "hunt", "glide", "climb", "swim", "roar", "pounce", "sprint", "chase",
        "race", "bound", "drift", "jump", "strut", "spin", "surge",
    ];

    // Each word list has 20 entries; consecutive counter values map to unique triples (a = idx % 20, n = (idx / 20) % 20,
    // v = (idx / 400) % 20), a bijection on 0..7999. A timestamp-derived offset varies the start across process restarts.
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);

    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    // Shift ts right to get a slowly-varying cross-run offset (changes every ~1 µs).
    let offset = (ts >> 10) % 8000;
    let idx = (count + offset) % 8000;

    let a = adjectives[(idx % 20) as usize];
    let n = nouns[((idx / 20) % 20) as usize];
    let v = verbs[((idx / 400) % 20) as usize];

    format!("{a}-{n}-{v}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use callisto_model::{ManifestDecl, ManifestFormat, ManifestRole, Package, PackageId, ReleaseTrigger};
    use std::collections::HashSet;
    use std::path::PathBuf;

    /// Answers the wizard from a script: each package list, then the summary, then the confirmation.
    #[derive(Default)]
    struct Scripted {
        /// `None` answers as Esc.
        selections: std::collections::VecDeque<Option<Vec<usize>>>,
        summary: Option<Summary>,
        confirm: bool,
        /// Esc at the summary or at the confirmation.
        esc_at_summary: bool,
        esc_at_confirm: bool,
        confirm_asked: bool,
        /// The names each list was shown, in order.
        shown: Vec<Vec<String>>,
        /// Which names each list marked.
        marked: Vec<Vec<bool>>,
        /// Whether each list accepted Enter with nothing picked.
        empty_ok: Vec<bool>,
        summary_asked: bool,
    }

    impl AddPrompter for Scripted {
        fn pick(
            &mut self,
            _heading: &str,
            items: &[Listed],
            marked: &[bool],
            empty_ok: bool,
        ) -> Result<Vec<usize>, CliError> {
            self.shown.push(items.iter().map(|item| item.name.clone()).collect());
            self.marked.push(marked.to_vec());
            self.empty_ok.push(empty_ok);
            self.selections
                .pop_front()
                .expect("an unexpected package list")
                .ok_or(CliError::Cancelled)
        }

        fn summary(&mut self, _prompt: &str) -> Result<Option<Summary>, CliError> {
            self.summary_asked = true;
            if self.esc_at_summary {
                return Err(CliError::Cancelled);
            }
            Ok(self.summary.clone())
        }

        fn confirm(&mut self, _prompt: &str, _default: bool) -> Result<bool, CliError> {
            self.confirm_asked = true;
            if self.esc_at_confirm {
                return Err(CliError::Cancelled);
            }
            Ok(self.confirm)
        }
    }

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    /// Answers each package list with these picks.
    fn picks<const N: usize>(answers: [Vec<usize>; N]) -> std::collections::VecDeque<Option<Vec<usize>>> {
        answers.into_iter().map(Some).collect()
    }

    /// Packages none of which changed since their last tag.
    fn unchanged(names: &[&str]) -> Vec<Choice> {
        names
            .iter()
            .map(|name| Choice {
                item: Listed::by_id(name),
                changed: false,
            })
            .collect()
    }

    #[test]
    fn changed_packages_are_listed_first_and_marked() {
        let choices = [
            ("cargo/a", false),
            ("cargo/b", true),
            ("cargo/c", false),
            ("cargo/d", true),
        ]
        .map(|(name, changed)| Choice {
            item: Listed::by_id(name),
            changed,
        });
        let mut prompter = Scripted {
            selections: picks([vec![0, 1, 2], vec![], vec![]]),
            summary: Some(Summary::new("Fix").unwrap()),
            confirm: true,
            ..Default::default()
        };
        let (entries, _) = run_wizard(&choices, None, false, &mut prompter)
            .unwrap()
            .expect("confirmed");
        assert_eq!(prompter.shown[0], names(&["cargo/b", "cargo/d", "cargo/a", "cargo/c"]));
        assert_eq!(prompter.marked[0], [true, true, false, false]);
        assert_eq!(prompter.shown[1], names(&["cargo/b", "cargo/d", "cargo/a"]));
        assert_eq!(prompter.marked[1], [false, false, false]);
        assert_eq!(
            prompter.empty_ok[..3],
            [false, true, true],
            "only the first list needs a pick, so only the bump lists offer Enter for none"
        );
        assert_eq!(
            severities(&entries),
            [
                ("cargo/b", Severity::Patch),
                ("cargo/d", Severity::Patch),
                ("cargo/a", Severity::Patch)
            ]
        );
    }

    #[test]
    fn esc_at_any_of_the_five_questions_cancels() {
        for at in 0..5 {
            let mut selections = picks([vec![0, 1], vec![], vec![]]);
            if at < 3 {
                selections[at] = None;
            }
            let mut prompter = Scripted {
                selections,
                summary: Some(Summary::new("Fix").unwrap()),
                confirm: true,
                esc_at_summary: at == 3,
                esc_at_confirm: at == 4,
                ..Default::default()
            };
            assert!(
                matches!(
                    run_wizard(&unchanged(&["cargo/a", "cargo/b"]), None, false, &mut prompter),
                    Err(CliError::Cancelled)
                ),
                "question {at}"
            );
            if at < 3 {
                assert_eq!(prompter.shown.len(), at + 1, "nothing is asked after Esc");
            }
            assert_eq!(prompter.summary_asked, at >= 3, "question {at}");
            assert_eq!(prompter.confirm_asked, at == 4, "question {at}");
        }
    }

    #[test]
    fn an_editor_that_saves_nothing_writes_nothing() {
        let mut prompter = Scripted {
            selections: picks([vec![0], vec![], vec![]]),
            summary: None,
            confirm: true,
            ..Default::default()
        };
        assert!(run_wizard(&unchanged(&["cargo/a"]), None, false, &mut prompter)
            .unwrap()
            .is_none());
    }

    #[test]
    fn a_dry_run_does_not_ask_to_write() {
        let mut prompter = Scripted {
            selections: picks([vec![0], vec![], vec![]]),
            summary: Some(Summary::new("Fix").unwrap()),
            confirm: false,
            ..Default::default()
        };
        let answers = run_wizard(&unchanged(&["cargo/a"]), None, true, &mut prompter).unwrap();
        assert!(answers.is_some());
        assert!(!prompter.confirm_asked);
    }

    #[test]
    fn the_review_lists_each_bump_then_the_summary() {
        let entries = [("cargo/a", Severity::Minor), ("cargo/b", Severity::Patch)].map(|(name, severity)| Entry {
            name: name.to_owned(),
            severity,
        });
        assert_eq!(
            review(&entries, "Add v2 exports\n\n- faster\n"),
            "\nThis changeset records:\n  minor  cargo/a\n  patch  cargo/b\n\n  Add v2 exports\n  \n  - faster\n"
        );
    }

    #[test]
    fn the_review_shows_a_control_character_in_a_name_escaped() {
        let entries = [Entry {
            name: "a\x1b[2Jb".to_owned(),
            severity: Severity::Patch,
        }];
        let text = review(&entries, "Fix");
        assert!(text.contains("a\\u{1b}[2Jb"), "{text:?}");
        assert!(!text.contains('\x1b'), "{text:?}");
    }

    fn severities(entries: &[Entry]) -> Vec<(&str, Severity)> {
        entries
            .iter()
            .map(|entry| (entry.name.as_str(), entry.severity))
            .collect()
    }

    #[test]
    fn the_wizard_gives_majors_then_minors_and_defaults_the_rest_to_patch() {
        let mut prompter = Scripted {
            selections: picks([vec![0, 1, 2], vec![0], vec![0]]),
            summary: Some(Summary::new("Rework the API").unwrap()),
            confirm: true,
            ..Default::default()
        };
        let (entries, summary) = run_wizard(
            &unchanged(&["cargo/a", "cargo/b", "cargo/c"]),
            None,
            false,
            &mut prompter,
        )
        .unwrap()
        .expect("confirmed");
        assert_eq!(
            severities(&entries),
            [
                ("cargo/a", Severity::Major),
                ("cargo/b", Severity::Minor),
                ("cargo/c", Severity::Patch)
            ]
        );
        assert_eq!(summary.0, "Rework the API");
        // The minor question is only about the packages that are not already major.
        assert_eq!(prompter.shown[2], names(&["cargo/b", "cargo/c"]));
    }

    #[test]
    fn the_wizard_only_offers_the_packages_that_were_selected_for_the_bump_questions() {
        let mut prompter = Scripted {
            selections: picks([vec![2, 0], vec![], vec![]]),
            summary: Some(Summary::new("Fix").unwrap()),
            confirm: true,
            ..Default::default()
        };
        let (entries, _) = run_wizard(
            &unchanged(&["cargo/a", "cargo/b", "cargo/c"]),
            None,
            false,
            &mut prompter,
        )
        .unwrap()
        .expect("confirmed");
        assert_eq!(
            severities(&entries),
            [("cargo/c", Severity::Patch), ("cargo/a", Severity::Patch)]
        );
        assert_eq!(prompter.shown[1], names(&["cargo/c", "cargo/a"]));
    }

    #[test]
    fn when_every_package_is_major_there_is_no_minor_question() {
        let mut prompter = Scripted {
            selections: picks([vec![0, 1], vec![0, 1]]),
            summary: Some(Summary::new("Breaking").unwrap()),
            confirm: true,
            ..Default::default()
        };
        run_wizard(&unchanged(&["cargo/a", "cargo/b"]), None, false, &mut prompter).unwrap();
        assert_eq!(prompter.shown.len(), 2, "no third list");
    }

    #[test]
    fn a_summary_given_on_the_command_line_is_not_asked_for_again() {
        let mut prompter = Scripted {
            selections: picks([vec![0], vec![], vec![]]),
            confirm: true,
            ..Default::default()
        };
        let (_, summary) = run_wizard(
            &unchanged(&["cargo/a"]),
            Some(Summary::new("From the flag").unwrap()),
            false,
            &mut prompter,
        )
        .unwrap()
        .expect("confirmed");
        assert_eq!(summary.0, "From the flag");
        assert!(!prompter.summary_asked);
    }

    #[test]
    fn declining_the_preview_cancels_without_a_changeset() {
        let mut prompter = Scripted {
            selections: picks([vec![0], vec![], vec![]]),
            summary: Some(Summary::new("Never mind").unwrap()),
            confirm: false,
            ..Default::default()
        };
        assert!(run_wizard(&unchanged(&["cargo/a"]), None, false, &mut prompter)
            .unwrap()
            .is_none());
    }

    #[test]
    fn a_workspace_with_no_packages_and_a_selection_of_none_are_errors_with_their_own_message() {
        let mut prompter = Scripted::default();
        assert!(matches!(
            run_wizard(&[], None, false, &mut prompter),
            Err(CliError::AddNoPackagesInWorkspace)
        ));
        let mut prompter = Scripted {
            selections: picks([vec![]]),
            ..Default::default()
        };
        assert!(matches!(
            run_wizard(&unchanged(&["cargo/a"]), None, false, &mut prompter),
            Err(CliError::AddNoPackagesSelected)
        ));
    }

    /// Constructs a minimal `Package` for a given id string (e.g. `"cargo/foo"`).
    fn make_package(id_str: &str) -> Package {
        let id = PackageId::parse(id_str).unwrap();
        let decl = ManifestDecl::new(
            PathBuf::from(format!("{}/Cargo.toml", id.name())),
            ManifestRole::Canonical,
            ManifestFormat::CargoToml,
        )
        .unwrap();
        Package {
            id,
            manifests: vec![decl],
            changelog: None,
            release_trigger: ReleaseTrigger::Changeset,
            publish_to: vec![],
            tag_template: None,
        }
    }

    #[test]
    fn a_blank_summary_is_rejected_and_any_other_is_trimmed() {
        for blank in ["", "   \t\n  "] {
            assert!(matches!(Summary::new(blank), Err(CliError::AddSummaryEmpty)));
        }
        assert_eq!(Summary::new("  important fix  ").unwrap().0, "important fix");
        assert!(matches!(Summary::new("\u{feff}  "), Err(CliError::AddSummaryEmpty)));
    }

    #[test]
    fn a_summary_loses_a_leading_byte_order_mark_and_crlf_line_ends() {
        assert_eq!(
            Summary::new("\u{feff}Fix b\r\n\r\n- one\r\n").unwrap().0,
            "Fix b\n\n- one"
        );
    }

    #[test]
    fn parse_severity_rejects_invalid_value_with_accurate_message() {
        let err = parse_severity("bogus").unwrap_err();
        assert_eq!(
            err.to_string(),
            "Invalid severity `bogus`. Must be none, patch, minor, or major."
        );
    }

    #[test]
    fn parse_severity_accepts_none() {
        assert_eq!(parse_severity("none").unwrap(), Severity::None);
    }

    /// Consecutive calls to generate_human_slug() must all produce distinct values,
    /// even when the OS clock resolution coalesces rapid calls to the same tick.
    #[test]
    fn test_generate_human_slug_consecutive_calls_differ() {
        let slugs: HashSet<String> = (0..50).map(|_| generate_human_slug()).collect();
        assert_eq!(
            slugs.len(),
            50,
            "expected 50 distinct slugs from 50 consecutive calls; got {} unique values",
            slugs.len()
        );
    }

    fn every_slug() -> HashSet<String> {
        let mut slugs = HashSet::new();
        for _ in 0..200_000 {
            slugs.insert(generate_human_slug());
            if slugs.len() == 8000 {
                break;
            }
        }
        assert_eq!(slugs.len(), 8000);
        slugs
    }

    #[test]
    fn an_existing_changeset_is_never_chosen_as_the_new_filename() {
        let dir = tempfile::tempdir().unwrap();
        let taken: Vec<String> = every_slug().into_iter().take(4000).collect();
        for slug in &taken {
            fs::write(dir.path().join(format!("{slug}.md")), "x").unwrap();
        }
        let chosen = unused_changeset_filename(dir.path(), &dir.path().join("pre")).unwrap();
        assert!(!dir.path().join(&chosen).exists(), "{chosen} already exists");
    }

    #[test]
    fn a_name_taken_after_it_was_chosen_is_skipped_and_the_other_changeset_survives() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("taken.md"), "someone else's").unwrap();
        let mut slugs = ["taken", "taken", "free"].into_iter().map(String::from);
        let chosen = create_changeset(
            dir.path(),
            &dir.path().join("pre"),
            "mine",
            &ApplyPermit::force_for_tests(),
            || slugs.next().unwrap(),
        )
        .unwrap();
        assert_eq!(chosen, "free.md");
        assert_eq!(
            fs::read_to_string(dir.path().join("taken.md")).unwrap(),
            "someone else's"
        );
        assert_eq!(fs::read_to_string(dir.path().join("free.md")).unwrap(), "mine");
    }

    #[test]
    fn a_name_a_pre_cycle_archived_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("pre");
        fs::create_dir_all(&archive).unwrap();
        fs::write(archive.join("taken.md"), "released this cycle").unwrap();
        let mut slugs = ["taken", "free"].into_iter().map(String::from);
        let chosen = create_changeset(dir.path(), &archive, "mine", &ApplyPermit::force_for_tests(), || {
            slugs.next().unwrap()
        })
        .unwrap();
        assert_eq!(chosen, "free.md");
        assert!(!dir.path().join("taken.md").exists());
    }

    #[test]
    fn a_directory_with_every_slug_taken_is_an_error_not_an_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        for slug in every_slug() {
            fs::write(dir.path().join(format!("{slug}.md")), "x").unwrap();
        }
        assert!(matches!(
            unused_changeset_filename(dir.path(), &dir.path().join("pre")),
            Err(CliError::Io { .. })
        ));
    }

    #[test]
    fn a_changesets_directory_that_does_not_exist_yet_gives_a_filename() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("not-created");
        assert!(unused_changeset_filename(&missing, &missing.join("pre"))
            .unwrap()
            .ends_with(".md"));
    }

    #[test]
    fn when_changed_packages_cannot_be_worked_out_every_package_is_offered_unmarked() {
        let packages = [make_package("npm/a"), make_package("npm/b")];
        let failed = Err(callisto_model::vcs::VcsError::Git("cat-file failed".to_owned()).into());
        let choices = wizard_choices(packages.iter(), failed);
        assert_eq!(choices.len(), 2);
        assert!(choices.iter().all(|choice| !choice.changed));
    }

    /// When a polyglot workspace contains `cargo/foo` and `npm/foo`, the
    /// interactive wizard's package-selection list must show two distinct,
    /// ecosystem-qualified names so the user can tell them apart.
    ///
    /// Pins `.display_name()`: `"cargo/foo"` and `"npm/foo"`, not two identical `"foo"` entries.
    #[test]
    fn polyglot_package_selection_names_are_distinct() {
        let packages = [make_package("cargo/foo"), make_package("npm/foo")];
        let names: Vec<String> = wizard_choices(packages.iter(), Ok(std::collections::BTreeMap::new()))
            .into_iter()
            .map(|choice| choice.item.name)
            .collect();

        assert_eq!(names.len(), 2, "expected 2 entries; got {names:?}");
        assert!(
            names.contains(&"cargo/foo".to_string()),
            "expected ecosystem-qualified name 'cargo/foo'; got {names:?}"
        );
        assert!(
            names.contains(&"npm/foo".to_string()),
            "expected ecosystem-qualified name 'npm/foo'; got {names:?}"
        );
    }
}
