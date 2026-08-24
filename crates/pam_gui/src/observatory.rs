use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pam_core::{CallerCredential, CallerId, ProjectId};
use pam_daemon::request_exchange;
use pam_platform::LocalEndpoint;
use pam_protocol::{
    ActivityResult, CallerListResult, ConnectorConfigureResult, ConnectorCredentialAction,
    ConnectorListResult, ConnectorTestResult, DaemonLogsResult, Failure, FailureCode,
    ModelGenerationResult, ModelMessage, ModelStatusResult, ProtocolContractError, RequestEnvelope,
    ResultBody, ResultPayload,
};

use crate::current::{unique_idempotency, unique_request_id};

const OBSERVATORY_TIMEOUT: Duration = Duration::from_secs(2);
const MODEL_INFER_DEADLINE: Duration = Duration::from_mins(2);
const MODEL_INFER_EXCHANGE_TIMEOUT: Duration = Duration::from_secs(125);
const MODEL_INFER_BLOCKED_RECOVERY: &str = "Grant the GUI caller the model.infer capability in PAM policy, or approve the pending model request.";
// The configure exchange includes daemon keychain writes; the test exchange
// wraps a daemon-side probe with its own ~10 second deadline.
const CONNECTOR_CONFIGURE_EXCHANGE_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECTOR_TEST_EXCHANGE_TIMEOUT: Duration = Duration::from_secs(15);
const CONNECTOR_CONFIGURE_BLOCKED_RECOVERY: &str = "Grant the GUI caller the connector.configure capability in PAM policy, or approve the pending connector request.";
const CONNECTOR_TEST_BLOCKED_RECOVERY: &str = "Grant the GUI caller the connector.test capability in PAM policy, or approve the pending connector request.";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ObservatoryState<T> {
    Available(T),
    Blocked {
        code: FailureCode,
        detail: String,
        recovery: Option<String>,
    },
    Unavailable {
        code: Option<String>,
        detail: String,
        recovery: Option<String>,
    },
}

pub(crate) async fn load_daemon_activity(
    caller_id: CallerId,
    credential: CallerCredential,
    project_id: ProjectId,
    limit: u32,
) -> ObservatoryState<ActivityResult> {
    let request = RequestEnvelope::daemon_activity(
        unique_request_id("gui-daemon-activity"),
        caller_id,
        project_id,
        unique_idempotency("gui-daemon-activity"),
        limit,
    )
    .authenticated(credential);
    load(
        request,
        "daemon-activity",
        OBSERVATORY_TIMEOUT,
        failure_state,
        |payload| match payload {
            ResultPayload::DaemonActivity(result) => Some(result),
            _ => None,
        },
    )
    .await
}

pub(crate) async fn load_daemon_logs(
    caller_id: CallerId,
    credential: CallerCredential,
    project_id: ProjectId,
    limit: u32,
) -> ObservatoryState<DaemonLogsResult> {
    let request = RequestEnvelope::daemon_logs(
        unique_request_id("gui-daemon-logs"),
        caller_id,
        project_id,
        unique_idempotency("gui-daemon-logs"),
        limit,
    )
    .authenticated(credential);
    load(
        request,
        "daemon-logs",
        OBSERVATORY_TIMEOUT,
        failure_state,
        |payload| match payload {
            ResultPayload::DaemonLogs(result) => Some(result),
            _ => None,
        },
    )
    .await
}

pub(crate) async fn load_caller_registry(
    caller_id: CallerId,
    credential: CallerCredential,
    project_id: ProjectId,
) -> ObservatoryState<CallerListResult> {
    let request = RequestEnvelope::caller_list(
        unique_request_id("gui-caller-registry"),
        caller_id,
        project_id,
        unique_idempotency("gui-caller-registry"),
    )
    .authenticated(credential);
    load(
        request,
        "caller-registry",
        OBSERVATORY_TIMEOUT,
        failure_state,
        |payload| match payload {
            ResultPayload::CallerList(result) => Some(result),
            _ => None,
        },
    )
    .await
}

pub(crate) async fn load_model_status(
    caller_id: CallerId,
    credential: CallerCredential,
    project_id: ProjectId,
) -> ObservatoryState<ModelStatusResult> {
    let request = RequestEnvelope::model_status(
        unique_request_id("gui-model-status"),
        caller_id,
        project_id,
        unique_idempotency("gui-model-status"),
    )
    .authenticated(credential);
    load(
        request,
        "model-status",
        OBSERVATORY_TIMEOUT,
        failure_state,
        |payload| match payload {
            ResultPayload::ModelStatus(result) => Some(result),
            _ => None,
        },
    )
    .await
}

/// Runs one policy-gated direct inference exchange.
///
/// # Errors
///
/// Returns the protocol contract error for an invalid model identity,
/// conversation shape, or output-token bound before any daemon exchange.
pub(crate) async fn run_model_infer(
    caller_id: CallerId,
    credential: CallerCredential,
    project_id: ProjectId,
    model: String,
    messages: Vec<ModelMessage>,
    max_output_tokens: u32,
) -> Result<ObservatoryState<ModelGenerationResult>, ProtocolContractError> {
    let deadline_unix_ms = now_unix_ms()
        .saturating_add(u64::try_from(MODEL_INFER_DEADLINE.as_millis()).unwrap_or(u64::MAX));
    let request = RequestEnvelope::model_infer(
        unique_request_id("gui-model-infer"),
        caller_id,
        project_id,
        unique_idempotency("gui-model-infer"),
        model,
        messages,
        max_output_tokens,
        deadline_unix_ms,
    )?
    .authenticated(credential);
    Ok(load(
        request,
        "model-infer",
        MODEL_INFER_EXCHANGE_TIMEOUT,
        infer_failure_state,
        |payload| match payload {
            ResultPayload::ModelGeneration(result) => Some(result),
            _ => None,
        },
    )
    .await)
}

pub(crate) async fn load_connector_registry(
    caller_id: CallerId,
    credential: CallerCredential,
    project_id: ProjectId,
) -> ObservatoryState<ConnectorListResult> {
    let request = RequestEnvelope::connector_list(
        unique_request_id("gui-connector-registry"),
        caller_id,
        project_id,
        unique_idempotency("gui-connector-registry"),
    )
    .authenticated(credential);
    load(
        request,
        "connector-registry",
        OBSERVATORY_TIMEOUT,
        failure_state,
        |payload| match payload {
            ResultPayload::ConnectorList(result) => Some(result),
            _ => None,
        },
    )
    .await
}

/// Runs one policy-gated connector configuration exchange.
///
/// The optional credential action passes through in memory only: it is never
/// logged, retained, or echoed by any result.
///
/// # Errors
///
/// Returns the protocol contract error for an invalid connector identity or
/// base URL before any daemon exchange.
pub(crate) async fn run_connector_configure(
    caller_id: CallerId,
    credential: CallerCredential,
    project_id: ProjectId,
    connector: String,
    enabled: Option<bool>,
    base_url: Option<String>,
    credential_action: Option<ConnectorCredentialAction>,
) -> Result<ObservatoryState<ConnectorConfigureResult>, ProtocolContractError> {
    let request = connector_configure_request(
        caller_id,
        project_id,
        connector,
        enabled,
        base_url,
        credential_action,
    )?
    .authenticated(credential);
    Ok(load(
        request,
        "connector-configure",
        CONNECTOR_CONFIGURE_EXCHANGE_TIMEOUT,
        connector_configure_failure_state,
        |payload| match payload {
            ResultPayload::ConnectorConfigure(result) => Some(result),
            _ => None,
        },
    )
    .await)
}

/// Runs one policy-gated connector self-test exchange.
///
/// # Errors
///
/// Returns the protocol contract error for an invalid connector identity
/// before any daemon exchange.
pub(crate) async fn run_connector_test(
    caller_id: CallerId,
    credential: CallerCredential,
    project_id: ProjectId,
    connector: String,
) -> Result<ObservatoryState<ConnectorTestResult>, ProtocolContractError> {
    let request = RequestEnvelope::connector_test(
        unique_request_id("gui-connector-test"),
        caller_id,
        project_id,
        unique_idempotency("gui-connector-test"),
        connector,
    )?
    .authenticated(credential);
    Ok(load(
        request,
        "connector-test",
        CONNECTOR_TEST_EXCHANGE_TIMEOUT,
        connector_test_failure_state,
        |payload| match payload {
            ResultPayload::ConnectorTest(result) => Some(result),
            _ => None,
        },
    )
    .await)
}

fn connector_configure_request(
    caller_id: CallerId,
    project_id: ProjectId,
    connector: String,
    enabled: Option<bool>,
    base_url: Option<String>,
    credential_action: Option<ConnectorCredentialAction>,
) -> Result<RequestEnvelope, ProtocolContractError> {
    RequestEnvelope::connector_configure(
        unique_request_id("gui-connector-configure"),
        caller_id,
        project_id,
        unique_idempotency("gui-connector-configure"),
        connector,
        enabled,
        base_url,
        credential_action,
    )
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

async fn load<T>(
    request: RequestEnvelope,
    surface: &str,
    timeout: Duration,
    classify: fn(Failure) -> ObservatoryState<T>,
    extract: fn(ResultPayload) -> Option<T>,
) -> ObservatoryState<T> {
    let exchange =
        match request_exchange(&LocalEndpoint::default_for_user(), &request, timeout).await {
            Ok(exchange) => exchange,
            Err(error) => {
                let (detail, recovery) = crate::control_center::exchange_failure_context(&error);
                return unavailable(detail, recovery);
            }
        };
    if !exchange.events.is_empty() {
        return unavailable(format!("PAM returned events for a {surface} read."), None);
    }
    match exchange.result.body {
        ResultBody::Success { payload, .. } => match extract(payload) {
            Some(result) => ObservatoryState::Available(result),
            None => unavailable(
                format!("PAM returned an unexpected {surface} response."),
                None,
            ),
        },
        ResultBody::Failure(failure) => classify(failure),
    }
}

fn unavailable<T>(detail: String, recovery: Option<String>) -> ObservatoryState<T> {
    ObservatoryState::Unavailable {
        code: None,
        detail,
        recovery,
    }
}

/// The observatory capabilities are baseline reads: an explicit policy deny
/// (or an unexpected approval demand) is blocked; everything else, including
/// an offline daemon, is unavailable.
fn failure_state<T>(failure: Failure) -> ObservatoryState<T> {
    if matches!(
        failure.code,
        FailureCode::Forbidden | FailureCode::ApprovalRequired
    ) {
        ObservatoryState::Blocked {
            code: failure.code,
            detail: failure.message,
            recovery: failure.recovery,
        }
    } else {
        ObservatoryState::Unavailable {
            code: None,
            detail: failure.message,
            recovery: failure.recovery,
        }
    }
}

/// `model.infer` is not a baseline capability: every policy or approval
/// refusal is blocked and always carries recovery text; everything else,
/// including an offline daemon, is unavailable.
fn infer_failure_state<T>(failure: Failure) -> ObservatoryState<T> {
    grant_failure_state(failure, MODEL_INFER_BLOCKED_RECOVERY)
}

/// `connector.configure` requires an explicit grant: classified exactly like
/// [`infer_failure_state`], with connector recovery text.
fn connector_configure_failure_state<T>(failure: Failure) -> ObservatoryState<T> {
    grant_failure_state(failure, CONNECTOR_CONFIGURE_BLOCKED_RECOVERY)
}

/// `connector.test` requires an explicit grant: classified exactly like
/// [`infer_failure_state`], with connector recovery text.
fn connector_test_failure_state<T>(failure: Failure) -> ObservatoryState<T> {
    grant_failure_state(failure, CONNECTOR_TEST_BLOCKED_RECOVERY)
}

fn grant_failure_state<T>(failure: Failure, default_recovery: &str) -> ObservatoryState<T> {
    if matches!(
        failure.code,
        FailureCode::Forbidden
            | FailureCode::ApprovalRequired
            | FailureCode::ApprovalDenied
            | FailureCode::ApprovalExpired
    ) {
        ObservatoryState::Blocked {
            code: failure.code,
            detail: failure.message,
            recovery: failure
                .recovery
                .or_else(|| Some(default_recovery.to_owned())),
        }
    } else {
        ObservatoryState::Unavailable {
            code: None,
            detail: failure.message,
            recovery: failure.recovery,
        }
    }
}

#[cfg(test)]
pub(crate) fn failure_state_for_test<T>(failure: Failure) -> ObservatoryState<T> {
    failure_state(failure)
}

#[cfg(test)]
pub(crate) fn infer_failure_state_for_test<T>(failure: Failure) -> ObservatoryState<T> {
    infer_failure_state(failure)
}

#[cfg(test)]
pub(crate) fn connector_configure_failure_state_for_test<T>(
    failure: Failure,
) -> ObservatoryState<T> {
    connector_configure_failure_state(failure)
}

#[cfg(test)]
pub(crate) fn connector_test_failure_state_for_test<T>(failure: Failure) -> ObservatoryState<T> {
    connector_test_failure_state(failure)
}

#[cfg(test)]
pub(crate) fn connector_configure_request_for_test(
    caller_id: CallerId,
    project_id: ProjectId,
    connector: String,
    enabled: Option<bool>,
    base_url: Option<String>,
    credential_action: Option<ConnectorCredentialAction>,
) -> Result<RequestEnvelope, ProtocolContractError> {
    connector_configure_request(
        caller_id,
        project_id,
        connector,
        enabled,
        base_url,
        credential_action,
    )
}
