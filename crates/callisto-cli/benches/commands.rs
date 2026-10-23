#![allow(clippy::unwrap_used, clippy::print_stdout)]
//! Wall time and git process counts for the commands that read a whole workspace.
//!
//! `just bench` runs this over generated workspaces. Set `CALLISTO_BENCH_PACKAGES=50,200` to choose the sizes and
//! `CALLISTO_BENCH_SHAPES=1` to list which git commands the processes were.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use callisto_fixtures::perf::ScaledWorkspace;

const RUNS: usize = 3;

const COMMANDS: &[(&str, &[&str])] = &[
    ("status", &["status"]),
    ("version --dry-run", &["--dry-run", "version"]),
    ("release --dry-run", &["--dry-run", "release"]),
];

struct Measured {
    wall: Duration,
    git_processes: usize,
    /// Time the git processes ran, from the `exit` events in git's trace.
    git_time: Duration,
    ok: bool,
    first_error_line: String,
    /// How many git processes ran per command shape (the subcommand and its leading flags).
    shapes: std::collections::BTreeMap<String, usize>,
    /// Seconds each command shape ran, summed over its processes.
    shape_seconds: std::collections::BTreeMap<String, f64>,
}

/// How long a git process ran, when `line` is its `exit` event.
fn exit_seconds(line: &str) -> Option<f64> {
    let event: serde_json::Value = serde_json::from_str(line).ok()?;
    if event.get("event")?.as_str()? != "exit" {
        return None;
    }
    event.get("t_abs")?.as_f64()
}

/// The git command a trace line started, without the revisions and paths that vary per package.
fn shape_of(line: &str) -> Option<String> {
    let event: serde_json::Value = serde_json::from_str(line).ok()?;
    if event.get("event")?.as_str()? != "start" {
        return None;
    }
    let argv = event.get("argv")?.as_array()?;
    let words: Vec<&str> = argv
        .iter()
        .skip(1)
        .filter_map(|word| word.as_str())
        .take_while(|word| *word != "--")
        .filter(|word| word.starts_with('-') || word.chars().all(|c| c.is_ascii_lowercase() || c == '-'))
        .take(4)
        .collect();
    Some(words.join(" "))
}

fn measure(root: &Path, args: &[&str], trace: &Path) -> Measured {
    drop(std::fs::remove_file(trace));
    let started = Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_callisto"))
        .arg("--cwd")
        .arg(root)
        .args(args)
        .env("GIT_TRACE2_EVENT", trace)
        .output()
        .unwrap();
    let wall = started.elapsed();
    let trace_text = std::fs::read_to_string(trace).unwrap_or_default();
    let mut shapes = std::collections::BTreeMap::new();
    for shape in trace_text.lines().filter_map(shape_of) {
        *shapes.entry(shape).or_insert(0) += 1;
    }
    let git_time = Duration::from_secs_f64(trace_text.lines().filter_map(exit_seconds).sum());
    let mut shape_by_session = std::collections::BTreeMap::new();
    let mut shape_seconds = std::collections::BTreeMap::new();
    for line in trace_text.lines() {
        let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let session = event
            .get("sid")
            .and_then(|sid| sid.as_str())
            .unwrap_or_default()
            .to_owned();
        if let Some(shape) = shape_of(line) {
            shape_by_session.insert(session, shape);
        } else if let (Some(seconds), Some(shape)) = (exit_seconds(line), shape_by_session.get(&session)) {
            *shape_seconds.entry(shape.clone()).or_insert(0.0) += seconds;
        }
    }
    Measured {
        wall,
        git_processes: shapes.values().sum(),
        git_time,
        shapes,
        shape_seconds,
        ok: output.status.success(),
        first_error_line: String::from_utf8_lossy(&output.stderr)
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or_default()
            .to_owned(),
    }
}

fn main() {
    let sizes: Vec<usize> = std::env::var("CALLISTO_BENCH_PACKAGES")
        .ok()
        .map(|raw| raw.split(',').filter_map(|part| part.trim().parse().ok()).collect())
        .unwrap_or_else(|| vec![50, 200, 500]);

    println!("| packages | command | median ms | git ms | git processes | result |");
    println!("| ---: | --- | ---: | ---: | ---: | --- |");
    for packages in sizes {
        let dir = tempfile::tempdir().unwrap();
        ScaledWorkspace {
            packages,
            extra_commits: packages * 2,
        }
        .build(dir.path());
        let trace = dir
            .path()
            .join("..")
            .join(format!("callisto-bench-trace-{packages}.json"));
        for (label, args) in COMMANDS {
            let mut runs: Vec<Measured> = (0..RUNS).map(|_| measure(dir.path(), args, &trace)).collect();
            runs.sort_by_key(|run| run.wall);
            let median = &runs[RUNS / 2];
            let result = if median.ok {
                "ok".to_owned()
            } else {
                format!("failed: {}", median.first_error_line)
            };
            println!(
                "| {packages} | {label} | {} | {} | {} | {result} |",
                median.wall.as_millis(),
                median.git_time.as_millis(),
                median.git_processes
            );
            if std::env::var_os("CALLISTO_BENCH_SHAPES").is_some() {
                let mut shapes: Vec<_> = median.shapes.iter().collect();
                shapes.sort_by(|(left, _), (right, _)| {
                    let seconds = |shape: &&String| median.shape_seconds.get(*shape).copied().unwrap_or(0.0);
                    seconds(right).total_cmp(&seconds(left))
                });
                for (shape, count) in shapes.into_iter().take(8) {
                    let ms = median.shape_seconds.get(shape).copied().unwrap_or(0.0) * 1000.0;
                    println!("|  |  |  | {ms:.0} | {count} x | `git {shape}` |  |");
                }
            }
        }
        drop(std::fs::remove_file(&trace));
    }
}
