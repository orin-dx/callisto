//! Pins the wording of main.rs's `status` exit-code-contract doc comment
//! against check_exit_code_raw's actual behavior (AC-05).

const MAIN_RS: &str = include_str!("../src/main.rs");

#[test]
fn status_exit_code_doc_comment_does_not_claim_zero_for_check_with_pending_changesets() {
    assert!(
        !MAIN_RS.contains("0 when no errors and (no --check OR at least one pending changeset)"),
        "main.rs's status exit-code doc comment must not claim 0 is returned under --check \
         when changesets are pending; check_exit_code_raw (status.rs) returns 1 in that case"
    );
}

#[test]
fn status_exit_code_doc_comment_states_correct_check_contract() {
    assert!(
        MAIN_RS.contains("OR when --check is set and at least one package has pending changesets"),
        "main.rs's status exit-code doc comment must state --check returns 1 when at least \
         one package has pending changesets"
    );
    assert!(
        MAIN_RS.contains("when --check is set, there are no diagnostic"),
        "main.rs's status exit-code doc comment must state --check returns 2 only when there \
         are no diagnostic errors and no changesets are pending"
    );
}
