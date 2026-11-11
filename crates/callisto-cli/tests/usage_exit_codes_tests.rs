//! Usage errors exit 2 and runtime failures exit 1, for text and JSON output alike.
#[path = "common/release_harness.rs"]
mod release_harness;

use std::collections::BTreeSet;
use std::fs;
use std::process::Command;

use callisto_cli::error::{USAGE_CODES, USAGE_WHEN_FROM_FLAG_CODES};
use release_harness::*;

/// Every other code in docs/errors.md: workspace, config, git, registry, network or release state.
const RUNTIME_CODES: &str = "
E001 E002 E003 E004 E005 E006 E007 E009 E010 E011 E012 E013 E014 E015 E016 E017 E018 E019 E020 E021 E022 E023
E024 E025 E026 E027 E028 E029 E030 E031 E032 E033 E034 E035 E036 E037 E038 E039 E040 E041 E042 E043 E044 E045
E046 E047 E051 E052 E053 E054 E055 E056 E057 E058 E059 E060 E061 E062 E063 E100 E101 E102 E103 E104 E105 E106
E107 E108 E109 E110 E111 E113 E114 E115 E116 E117 E118 E119 E120 E121 E122 E123 E124 E125 E126 E127 E132 E136
E137 E138 E139 E140 E141 E142 E143 E144 E145 E146 E147 E148 E149 E150 E151 E152 E153 E154 E155 E157 E158 E159
E160 E161 E162 E163 E164 E165 E166 E167 E168 E169 E170 E171 E172 E175 E176 E177 E178 E179 E180 E187 E188 E189
E190 E191 E193 E194 E197 E198 E199 E200 E201 E202 E203 E204 E205 E206 E207 E208 E209 E210 E211 E212 E213 E215
E219 E220 E221 E222 E223 E230 E232 E233 E234 E235 E236 E237 E238 E239 E243 E244 E245 E246 E247 E249 E250 E251
E252 E255 E256 E259 E260 E261 E262 E263 E264 E265 E266 E267 E268 E269 E272 E273 E277 E278 E280 E281 E284
E285 E286 E287 E288 E289 E290 E291 E292 E293 E294 E295 E296 E297 E299 E300 E301 E302 E303 E304 E305 E306 E307
E308 E310 E311 E312 E313 E314 E315 E318 E319 E320 E321 E322 E323 E325 E326 E328
E330 E331 E332 E333 E334 E335 E336 E337 E338 E339
";

fn documented_codes() -> BTreeSet<String> {
    let docs = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/errors.md")).unwrap();
    docs.lines()
        .filter_map(|line| line.strip_prefix("| E"))
        .filter_map(|rest| rest.split_once(' ').map(|(digits, _)| format!("E{digits}")))
        .collect()
}

#[test]
fn every_documented_code_is_classified_usage_or_runtime_exactly_once() {
    let mut seen = BTreeSet::new();
    let classified = USAGE_CODES
        .iter()
        .chain(USAGE_WHEN_FROM_FLAG_CODES)
        .copied()
        .chain(RUNTIME_CODES.split_whitespace());
    for code in classified {
        assert!(seen.insert(code.to_string()), "{code} is classified twice");
    }
    let documented = documented_codes();
    let unclassified: Vec<_> = documented.difference(&seen).collect();
    assert!(
        unclassified.is_empty(),
        "classify as usage (error.rs) or runtime (this file): {unclassified:?}"
    );
    let stale: Vec<_> = seen.difference(&documented).collect();
    assert!(stale.is_empty(), "classified but not in docs/errors.md: {stale:?}");
}

fn workspace() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    callisto_fixtures::git::init_repo(root);
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"a\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), "").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "workspace"]);
    dir
}

fn assert_exit(dir: &std::path::Path, args: &[&str], exit: i32, code: &str) {
    let json = callisto(dir, args);
    assert_eq!(json.status.code(), Some(exit), "json {args:?}: {}", stderr_of(&json));
    // `init` prints what it detected before the envelope.
    let stderr = stderr_of(&json);
    let body = stderr.find("{\n").map_or(stderr.as_str(), |start| &stderr[start..]);
    let envelope: serde_json::Value = serde_json::from_str(body)
        .unwrap_or_else(|error| panic!("{args:?} stderr has no JSON envelope ({error}): {stderr}"));
    assert_eq!(envelope["error"]["code"], code, "{args:?}: {envelope}");

    let text = Command::new(env!("CARGO_BIN_EXE_callisto"))
        .args(["--cwd", dir.to_str().unwrap()])
        .args(args)
        .output()
        .unwrap();
    assert_eq!(text.status.code(), Some(exit), "text {args:?}: {}", stderr_of(&text));
}

#[test]
fn bad_or_missing_flag_values_checked_after_parsing_exit_2() {
    let dir = workspace();
    let root = dir.path();
    for (args, code) in [
        (vec!["add", "--package", "a:patch", "--summary", "s"], "E317"),
        (vec!["add", "--package", "cargo/a:bogus", "--summary", "s"], "E271"),
        (vec!["add", "--package", "cargo/a:patch"], "E274"),
        (vec!["add", "--package", "cargo/a:patch", "--summary", ""], "E275"),
        (vec!["add"], "E214"),
        (vec!["init"], "E240"),
        (vec!["init", "--workflow", "--no-workflow"], "E242"),
        (vec!["snapshot", "--tag", "a/b c"], "E324"),
        (vec!["pre", "enter", " "], "E279"),
    ] {
        assert_exit(root, &args, 2, code);
    }
}

#[test]
fn runtime_failures_still_exit_1() {
    let dir = workspace();
    let root = dir.path();
    assert_exit(
        root,
        &["add", "--package", "cargo/nope:patch", "--summary", "s"],
        1,
        "E102",
    );
    fs::write(root.join("callisto.toml"), "not = [valid").unwrap();
    assert_exit(root, &["status"], 1, "E111");
}

#[test]
fn a_bare_package_in_config_exits_1_though_the_same_code_on_a_flag_exits_2() {
    let dir = workspace();
    let root = dir.path();
    fs::write(root.join("callisto.toml"), "[[package]]\nmatch = \"a\"\n").unwrap();
    assert_exit(root, &["status"], 1, "E317");
}
