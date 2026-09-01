use pam_core::{CallerId, ProjectId};
use pam_protocol::{
    ActivityResult, ConnectorConfigureResult, ConnectorCredentialAction, ConnectorSecret,
    ConnectorTestResult, Failure, FailureCode, ModelGenerationResult, RequestPayload,
};

use super::observatory::{
    ObservatoryState, connector_configure_failure_state_for_test,
    connector_configure_request_for_test, connector_test_failure_state_for_test,
    failure_state_for_test, infer_failure_state_for_test,
};

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
        Some("Start the Pam daemon.".to_owned()),
    ));

    assert_eq!(
        state,
        ObservatoryState::Unavailable {
            code: None,
            detail: "observed failure".to_owned(),
            recovery: Some("Start the Pam daemon.".to_owned()),
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
fn connector_grant_refusals_are_blocked_and_always_carry_recovery() {
    for code in [
        FailureCode::Forbidden,
        FailureCode::ApprovalRequired,
        FailureCode::ApprovalDenied,
        FailureCode::ApprovalExpired,
    ] {
        let configure: ObservatoryState<ConnectorConfigureResult> =
            connector_configure_failure_state_for_test(failure(code.clone(), None));
        let test: ObservatoryState<ConnectorTestResult> =
            connector_test_failure_state_for_test(failure(code.clone(), None));
        let ObservatoryState::Blocked { recovery, .. } = configure else {
            panic!("refusal {code:?} must be blocked");
        };
        assert!(
            recovery.is_some_and(|text| text.contains("connector.configure")),
            "configure refusal {code:?} must carry connector recovery"
        );
        let ObservatoryState::Blocked { recovery, .. } = test else {
            panic!("refusal {code:?} must be blocked");
        };
        assert!(
            recovery.is_some_and(|text| text.contains("connector.test")),
            "test refusal {code:?} must carry connector recovery"
        );
    }
}

#[test]
fn connector_transport_failures_are_unavailable() {
    let state: ObservatoryState<ConnectorTestResult> =
        connector_test_failure_state_for_test(failure(
            FailureCode::Internal,
            Some("Start the Pam daemon.".to_owned()),
        ));

    assert_eq!(
        state,
        ObservatoryState::Unavailable {
            code: None,
            detail: "observed failure".to_owned(),
            recovery: Some("Start the Pam daemon.".to_owned()),
        }
    );
}

#[test]
fn connector_configure_maps_credential_actions_through_without_retention_or_debug_exposure() {
    let secret = ConnectorSecret::new("token-value-1234").unwrap();
    let request = connector_configure_request_for_test(
        CallerId::from("gui"),
        ProjectId::from("project-7"),
        "github-actions".to_owned(),
        Some(true),
        Some("https://api.github.com".to_owned()),
        Some(ConnectorCredentialAction::Set {
            secret: secret.clone(),
        }),
    )
    .unwrap();

    let RequestPayload::ConnectorConfigure {
        connector,
        enabled,
        base_url,
        credential,
    } = &request.payload
    else {
        panic!("the request must carry the connector configure payload");
    };
    assert_eq!(connector, "github-actions");
    assert_eq!(enabled, &Some(true));
    assert_eq!(base_url.as_deref(), Some("https://api.github.com"));
    assert_eq!(credential, &Some(ConnectorCredentialAction::Set { secret }));
    let debugged = format!("{request:?}");
    assert!(debugged.contains("[REDACTED]"));
    assert!(!debugged.contains("token-value-1234"));

    let cleared = connector_configure_request_for_test(
        CallerId::from("gui"),
        ProjectId::from("project-7"),
        "github-actions".to_owned(),
        None,
        None,
        Some(ConnectorCredentialAction::Clear),
    )
    .unwrap();
    assert!(matches!(
        &cleared.payload,
        RequestPayload::ConnectorConfigure {
            enabled: None,
            base_url: None,
            credential: Some(ConnectorCredentialAction::Clear),
            ..
        }
    ));

    let invalid = connector_configure_request_for_test(
        CallerId::from("gui"),
        ProjectId::from("project-7"),
        "Not A Connector Id".to_owned(),
        None,
        None,
        None,
    );
    assert!(invalid.is_err());
}

#[test]
fn infer_transport_failures_are_unavailable() {
    let state: ObservatoryState<ModelGenerationResult> = infer_failure_state_for_test(failure(
        FailureCode::Internal,
        Some("Start the Pam daemon.".to_owned()),
    ));

    assert_eq!(
        state,
        ObservatoryState::Unavailable {
            code: None,
            detail: "observed failure".to_owned(),
            recovery: Some("Start the Pam daemon.".to_owned()),
        }
    );
}
