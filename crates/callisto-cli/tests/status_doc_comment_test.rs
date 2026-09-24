//! Pins the wording of main.rs's `status` exit-code-contract doc comment
//! against check_exit_code_raw's actual behavior (SPEC-DX-STATUS-ADD AC-04).

const MAIN_RS: &str = include_str!("../src/main.rs");

#[test]
fn status_exit_code_doc_comment_does_not_claim_zero_for_check_with_pending_changesets() {
    assert!(
        !MAIN_RS.contains("0 when no errors and (no --check OR at least one pending changeset)"),
        "main.rs's status exit-code doc comment must not claim 0 is returned under --check \
         when changesets are pending; check_exit_code_raw (status.rs) never returns 0"
    );
}

#[test]
fn status_exit_code_doc_comment_states_correct_check_contract() {
    assert!(
        MAIN_RS.contains("regardless of --check or pending changesets"),
        "main.rs's status exit-code doc comment must state errors return 1 regardless of \
         --check or pending changesets"
    );
    assert!(
        MAIN_RS.contains("when --check is set, there are no diagnostic\n//                    errors, and at least one changeset is pending"),
        "main.rs's status exit-code doc comment must state --check returns 2 when there are \
         no diagnostic errors and at least one changeset is pending"
    );
    assert!(
        MAIN_RS.contains(
            "when --check is set, there are no diagnostic\n//                    errors, and nothing is pending"
        ),
        "main.rs's status exit-code doc comment must state --check returns 3 when there are \
         no diagnostic errors and nothing is pending"
    );
}
