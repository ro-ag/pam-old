use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pam_core::{CallerCredential, CallerId, ProjectId};
use pam_daemon::request_exchange;
use pam_platform::LocalEndpoint;
use pam_protocol::{
    ActivityResult, CallerListResult, Failure, FailureCode, ModelGenerationResult, ModelMessage,
    ModelStatusResult, ProtocolContractError, RequestEnvelope, ResultBody, ResultPayload,
};

use crate::current::{unique_idempotency, unique_request_id};

const OBSERVATORY_TIMEOUT: Duration = Duration::from_secs(2);
const MODEL_INFER_DEADLINE: Duration = Duration::from_mins(2);
const MODEL_INFER_EXCHANGE_TIMEOUT: Duration = Duration::from_secs(125);
const MODEL_INFER_BLOCKED_RECOVERY: &str = "Grant the GUI caller the model.infer capability in PAM policy, or approve the pending model request.";

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
                return unavailable(
                    error.to_string(),
                    error.recovery_action().map(str::to_owned),
                );
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
                .or_else(|| Some(MODEL_INFER_BLOCKED_RECOVERY.to_owned())),
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
