use pam_protocol::{ActivityResult, Failure, FailureCode, ModelGenerationResult};

use super::observatory::{ObservatoryState, failure_state_for_test, infer_failure_state_for_test};

fn failure(code: FailureCode, recovery: Option<String>) -> Failure {
    Failure {
        code,
        message: "observed failure".to_owned(),
        recovery,
        approval: None,
    }
}

#[test]
fn explicit_policy_denials_are_blocked() {
    for code in [FailureCode::Forbidden, FailureCode::ApprovalRequired] {
        let state: ObservatoryState<ActivityResult> =
            failure_state_for_test(failure(code.clone(), None));
        assert!(
            matches!(state, ObservatoryState::Blocked { code: observed, .. } if observed == code)
        );
    }
}

#[test]
fn non_policy_failures_are_unavailable_and_keep_recovery_text() {
    let state: ObservatoryState<ActivityResult> = failure_state_for_test(failure(
        FailureCode::Internal,
        Some("Start the PAM daemon.".to_owned()),
    ));

    assert_eq!(
        state,
        ObservatoryState::Unavailable {
            code: None,
            detail: "observed failure".to_owned(),
            recovery: Some("Start the PAM daemon.".to_owned()),
        }
    );
}

#[test]
fn infer_policy_and_approval_refusals_are_blocked_and_always_carry_recovery() {
    for code in [
        FailureCode::Forbidden,
        FailureCode::ApprovalRequired,
        FailureCode::ApprovalDenied,
        FailureCode::ApprovalExpired,
    ] {
        let state: ObservatoryState<ModelGenerationResult> =
            infer_failure_state_for_test(failure(code.clone(), None));
        let ObservatoryState::Blocked {
            code: observed,
            recovery,
            ..
        } = state
        else {
            panic!("refusal {code:?} must be blocked");
        };
        assert_eq!(observed, code);
        assert!(recovery.is_some(), "refusal {code:?} must carry recovery");
    }
}

#[test]
fn infer_refusals_keep_the_daemon_recovery_text_when_present() {
    let state: ObservatoryState<ModelGenerationResult> = infer_failure_state_for_test(failure(
        FailureCode::ApprovalDenied,
        Some("Approve the pending model request.".to_owned()),
    ));

    assert!(matches!(
        state,
        ObservatoryState::Blocked { recovery: Some(recovery), .. }
            if recovery == "Approve the pending model request."
    ));
}

#[test]
fn infer_transport_failures_are_unavailable() {
    let state: ObservatoryState<ModelGenerationResult> = infer_failure_state_for_test(failure(
        FailureCode::Internal,
        Some("Start the PAM daemon.".to_owned()),
    ));

    assert_eq!(
        state,
        ObservatoryState::Unavailable {
            code: None,
            detail: "observed failure".to_owned(),
            recovery: Some("Start the PAM daemon.".to_owned()),
        }
    );
}
