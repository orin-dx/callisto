//! Pins the wording of main.rs's `status` exit-code-contract doc comment
//! against `has_error_diagnostics`'s actual behavior (SPEC-DX-STATUS-ADD AC-04).

const MAIN_RS: &str = include_str!("../src/main.rs");

#[test]
fn status_exit_code_doc_comment_does_not_claim_pending_state_affects_exit_code() {
    assert!(
        !MAIN_RS.contains("2 (ExitCode::from(2))") && !MAIN_RS.contains("3 (ExitCode::from(3))"),
        "main.rs's status exit-code doc comment must not describe a 2/3 pending-state \
         scheme; `status --check` is a conventional errors-only 0/1 gate"
    );
}

#[test]
fn status_exit_code_doc_comment_states_correct_check_contract() {
    assert!(
        MAIN_RS.contains("status without --check is informational, never a gate"),
        "main.rs's status exit-code doc comment must state plain status always exits 0 \
         (barring a command failure), regardless of diagnostics"
    );
    assert!(
        MAIN_RS.contains("1 when --check is set and any diagnostic is Error-severity"),
        "main.rs's status exit-code doc comment must state --check returns 1 only when \
         an Error-severity diagnostic is present"
    );
    assert!(
        MAIN_RS.contains("pending state never affects the exit code"),
        "main.rs's status exit-code doc comment must state pending changesets never \
         affect the exit code"
    );
}
