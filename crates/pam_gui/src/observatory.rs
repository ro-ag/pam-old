use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pam_client::request_exchange;
use pam_core::{CallerCredential, CallerId, ProjectId};
use pam_platform::LocalEndpoint;
use pam_protocol::{
    ActivityResult, CallerListResult, ConnectorConfigureResult, ConnectorCredentialAction,
    ConnectorListResult, ConnectorTestResult, DaemonLogsResult, DaemonStatsResult, Failure,
    FailureCode, GrantRevokeResult, ModelDeleteWeightsResult, ModelGenerationResult,
    ModelLoadResult, ModelMessage, ModelRegisterResult, ModelRegistration, ModelStatusResult,
    ModelSweepResult, ModelUnloadResult, ModelUnregisterResult, ModelVerifyResult,
    ProtocolContractError, RequestEnvelope, ResetResult, ResetTier, ResultBody, ResultPayload,
};

use crate::current::{unique_idempotency, unique_request_id};

const OBSERVATORY_TIMEOUT: Duration = Duration::from_secs(2);
const MODEL_INFER_DEADLINE: Duration = Duration::from_mins(2);
const MODEL_INFER_EXCHANGE_TIMEOUT: Duration = Duration::from_secs(125);
const MODEL_INFER_BLOCKED_RECOVERY: &str = "Grant the GUI caller the model.infer capability in Pam policy, or approve the pending model request.";
// The configure exchange includes daemon keychain writes; the test exchange
// wraps a daemon-side probe with its own ~10 second deadline.
const CONNECTOR_CONFIGURE_EXCHANGE_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECTOR_TEST_EXCHANGE_TIMEOUT: Duration = Duration::from_secs(15);
// Registration writes one durable registry row; revocation writes at most a
// handful of grant rows. Both are local store writes behind a short exchange.
const MODEL_REGISTER_EXCHANGE_TIMEOUT: Duration = Duration::from_secs(10);
const GRANT_REVOKE_EXCHANGE_TIMEOUT: Duration = Duration::from_secs(10);
const MODEL_REGISTER_BLOCKED_RECOVERY: &str = "Grant the GUI caller the model.register capability in Access, or approve the pending registration.";
const MODEL_UNREGISTER_BLOCKED_RECOVERY: &str = "Grant the GUI caller the model.unregister capability in Access, or approve the pending removal.";
const MODEL_LOAD_BLOCKED_RECOVERY: &str =
    "Grant the GUI caller the model.load capability in Access, or approve the pending load.";
const MODEL_UNLOAD_BLOCKED_RECOVERY: &str =
    "Grant the GUI caller the model.unload capability in Access, or approve the pending unload.";
/// Loading hashes and maps a multi-gigabyte artifact, and unloading waits for
/// the outgoing model to finish draining. Neither is a status read, so both
/// get the window verification gets rather than a write's short one.
const MODEL_LOAD_EXCHANGE_TIMEOUT: Duration = Duration::from_mins(10);
const MODEL_VERIFY_BLOCKED_RECOVERY: &str =
    "Grant the GUI caller the model.verify capability in Access, or approve the pending check.";
const MODEL_SWEEP_BLOCKED_RECOVERY: &str =
    "Grant the GUI caller the model.sweep capability in Access, or approve the pending sweep.";
const MODEL_DELETE_WEIGHTS_BLOCKED_RECOVERY: &str = "Grant the GUI caller the model.delete-weights capability in Access, or approve the pending deletion.";
/// Verification re-hashes every registered artifact, which for a catalog of
/// multi-gigabyte weights is minutes of disk work, not a status read.
const MODEL_VERIFY_EXCHANGE_TIMEOUT: Duration = Duration::from_mins(10);
// A reset walks the whole store, and clearing history also unlinks every
// evidence blob, so it gets a far longer window than an ordinary write.
const RESET_EXCHANGE_TIMEOUT: Duration = Duration::from_mins(1);
const RESET_BLOCKED_RECOVERY: &str =
    "Grant the GUI caller this reset capability in Access, or approve the pending reset.";
const CONNECTOR_CONFIGURE_BLOCKED_RECOVERY: &str = "Grant the GUI caller the connector.configure capability in Pam policy, or approve the pending connector request.";
const CONNECTOR_TEST_BLOCKED_RECOVERY: &str = "Grant the GUI caller the connector.test capability in Pam policy, or approve the pending connector request.";

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

pub(crate) async fn load_daemon_stats(
    caller_id: CallerId,
    credential: CallerCredential,
    project_id: ProjectId,
    days: u32,
) -> ObservatoryState<DaemonStatsResult> {
    let request = RequestEnvelope::daemon_stats(
        unique_request_id("gui-daemon-stats"),
        caller_id,
        project_id,
        unique_idempotency("gui-daemon-stats"),
        days,
    )
    .authenticated(credential);
    load(
        request,
        "daemon-stats",
        OBSERVATORY_TIMEOUT,
        failure_state,
        |payload| match payload {
            ResultPayload::DaemonStats(result) => Some(result),
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

/// Registers one verified model through the daemon that owns the store.
///
/// # Errors
///
/// Returns the protocol contract error for a registration the contract
/// rejects, before any daemon exchange.
pub(crate) async fn run_model_register(
    caller_id: CallerId,
    credential: CallerCredential,
    registration: ModelRegistration,
) -> Result<ObservatoryState<ModelRegisterResult>, ProtocolContractError> {
    let request = RequestEnvelope::model_register(
        unique_request_id("gui-model-register"),
        caller_id,
        ProjectId::daemon_scope(),
        unique_idempotency("gui-model-register"),
        registration,
    )?
    .authenticated(credential);
    Ok(load(
        request,
        "model-register",
        MODEL_REGISTER_EXCHANGE_TIMEOUT,
        model_register_failure_state,
        |payload| match payload {
            ResultPayload::ModelRegister(result) => Some(result),
            _ => None,
        },
    )
    .await)
}

/// Brings one registered model into the running daemon, without a restart.
///
/// The daemon owns the swap and does it old-before-new, so the GUI never has
/// to stop and restart Pam to change models. A load that fails leaves the
/// daemon serving without a model and arrives here as unavailable, carrying
/// the daemon's own reason and recovery line.
///
/// # Errors
///
/// Returns the protocol contract error for a model identity the contract
/// rejects, before any daemon exchange.
pub(crate) async fn run_model_load(
    caller_id: CallerId,
    credential: CallerCredential,
    model: String,
) -> Result<ObservatoryState<ModelLoadResult>, ProtocolContractError> {
    let request = RequestEnvelope::model_load(
        unique_request_id("gui-model-load"),
        caller_id,
        ProjectId::daemon_scope(),
        unique_idempotency("gui-model-load"),
        model,
    )?
    .authenticated(credential);
    Ok(load(
        request,
        "model-load",
        MODEL_LOAD_EXCHANGE_TIMEOUT,
        model_load_failure_state,
        |payload| match payload {
            ResultPayload::ModelLoad(result) => Some(result),
            _ => None,
        },
    )
    .await)
}

/// Drops the model the daemon holds and frees its memory, leaving it serving.
///
/// The registry is untouched, so the same model loads again without
/// re-importing anything.
pub(crate) async fn run_model_unload(
    caller_id: CallerId,
    credential: CallerCredential,
) -> ObservatoryState<ModelUnloadResult> {
    let request = RequestEnvelope::model_unload(
        unique_request_id("gui-model-unload"),
        caller_id,
        ProjectId::daemon_scope(),
        unique_idempotency("gui-model-unload"),
    )
    .authenticated(credential);
    load(
        request,
        "model-unload",
        MODEL_LOAD_EXCHANGE_TIMEOUT,
        model_unload_failure_state,
        |payload| match payload {
            ResultPayload::ModelUnload(result) => Some(result),
            _ => None,
        },
    )
    .await
}

/// Removes one model's registration through the daemon that owns the store.
///
/// The weights are never touched: this drops the registry row only.
///
/// # Errors
///
/// Returns the protocol contract error for a model identity the contract
/// rejects, before any daemon exchange.
pub(crate) async fn run_model_unregister(
    caller_id: CallerId,
    credential: CallerCredential,
    model: String,
) -> Result<ObservatoryState<ModelUnregisterResult>, ProtocolContractError> {
    let request = RequestEnvelope::model_unregister(
        unique_request_id("gui-model-unregister"),
        caller_id,
        ProjectId::daemon_scope(),
        unique_idempotency("gui-model-unregister"),
        model,
    )?
    .authenticated(credential);
    Ok(load(
        request,
        "model-unregister",
        MODEL_REGISTER_EXCHANGE_TIMEOUT,
        model_unregister_failure_state,
        |payload| match payload {
            ResultPayload::ModelUnregister(result) => Some(result),
            _ => None,
        },
    )
    .await)
}

/// Re-reads the registered weights and reports what still matches the registry.
///
/// This is the registry's health, not the loaded model's: it asks whether the
/// bytes on disk are still the bytes the registry recorded, and needs no model
/// to be loaded at all.
///
/// # Errors
///
/// Returns the protocol contract error for a model identity the contract
/// rejects, before any daemon exchange.
pub(crate) async fn run_model_verify(
    caller_id: CallerId,
    credential: CallerCredential,
    model: Option<String>,
) -> Result<ObservatoryState<ModelVerifyResult>, ProtocolContractError> {
    let request = RequestEnvelope::model_verify(
        unique_request_id("gui-model-verify"),
        caller_id,
        ProjectId::daemon_scope(),
        unique_idempotency("gui-model-verify"),
        model,
    )?
    .authenticated(credential);
    Ok(load(
        request,
        "model-verify",
        MODEL_VERIFY_EXCHANGE_TIMEOUT,
        model_verify_failure_state,
        |payload| match payload {
            ResultPayload::ModelVerify(result) => Some(result),
            _ => None,
        },
    )
    .await)
}

/// Reconciles the registry against the models directory, in both directions.
pub(crate) async fn run_model_sweep(
    caller_id: CallerId,
    credential: CallerCredential,
) -> ObservatoryState<ModelSweepResult> {
    let request = RequestEnvelope::model_sweep(
        unique_request_id("gui-model-sweep"),
        caller_id,
        ProjectId::daemon_scope(),
        unique_idempotency("gui-model-sweep"),
    )
    .authenticated(credential);
    load(
        request,
        "model-sweep",
        MODEL_REGISTER_EXCHANGE_TIMEOUT,
        model_sweep_failure_state,
        |payload| match payload {
            ResultPayload::ModelSweep(result) => Some(result),
            _ => None,
        },
    )
    .await
}

/// Deletes one Pam-downloaded model's weights and unregisters it.
///
/// The daemon owns the provenance gate: a GGUF Pam only ever verified in place
/// is refused, and the refusal arrives as unavailable carrying the daemon's
/// own explanation and recovery line.
///
/// # Errors
///
/// Returns the protocol contract error for a model identity the contract
/// rejects, before any daemon exchange.
pub(crate) async fn run_model_delete_weights(
    caller_id: CallerId,
    credential: CallerCredential,
    model: String,
) -> Result<ObservatoryState<ModelDeleteWeightsResult>, ProtocolContractError> {
    let request = RequestEnvelope::model_delete_weights(
        unique_request_id("gui-model-delete-weights"),
        caller_id,
        ProjectId::daemon_scope(),
        unique_idempotency("gui-model-delete-weights"),
        model,
    )?
    .authenticated(credential);
    Ok(load(
        request,
        "model-delete-weights",
        MODEL_REGISTER_EXCHANGE_TIMEOUT,
        model_delete_weights_failure_state,
        |payload| match payload {
            ResultPayload::ModelDeleteWeights(result) => Some(result),
            _ => None,
        },
    )
    .await)
}

/// Revokes every daemon-scope grant the GUI caller holds for one capability,/// Revokes every daemon-scope grant the GUI caller holds for one capability,
/// through the daemon that owns the store.
///
/// # Errors
///
/// Returns the protocol contract error for a capability name the contract
/// rejects, before any daemon exchange.
pub(crate) async fn run_grant_revoke(
    caller_id: CallerId,
    credential: CallerCredential,
    capability: String,
) -> Result<ObservatoryState<GrantRevokeResult>, ProtocolContractError> {
    let request = RequestEnvelope::grant_revoke(
        unique_request_id("gui-grant-revoke"),
        caller_id,
        ProjectId::daemon_scope(),
        unique_idempotency("gui-grant-revoke"),
        capability,
    )?
    .authenticated(credential);
    Ok(load(
        request,
        "grant-revoke",
        GRANT_REVOKE_EXCHANGE_TIMEOUT,
        failure_state,
        |payload| match payload {
            ResultPayload::GrantRevoke(result) => Some(result),
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

/// Runs one scoped reset tier, or forecasts it, through the daemon that owns
/// the store.
///
/// Reset is daemon-global, so this always speaks in the reserved daemon
/// scope: its grants live there and its refusals recover there.
pub(crate) async fn run_reset(
    caller_id: CallerId,
    credential: CallerCredential,
    tier: ResetTier,
    dry_run: bool,
) -> ObservatoryState<ResetResult> {
    let request = RequestEnvelope::reset(
        unique_request_id("gui-reset"),
        caller_id,
        ProjectId::daemon_scope(),
        unique_idempotency("gui-reset"),
        tier,
        dry_run,
    )
    .authenticated(credential);
    load(
        request,
        "reset",
        RESET_EXCHANGE_TIMEOUT,
        reset_failure_state,
        |payload| match payload {
            ResultPayload::Reset(result) => Some(result),
            _ => None,
        },
    )
    .await
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
        return unavailable(format!("Pam returned events for a {surface} read."), None);
    }
    match exchange.result.body {
        ResultBody::Success { payload, .. } => match extract(payload) {
            Some(result) => ObservatoryState::Available(result),
            None => unavailable(
                format!("Pam returned an unexpected {surface} response."),
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

/// `model.register` requires an explicit grant: classified exactly like
/// [`infer_failure_state`], with registration recovery text.
fn model_register_failure_state<T>(failure: Failure) -> ObservatoryState<T> {
    grant_failure_state(failure, MODEL_REGISTER_BLOCKED_RECOVERY)
}

/// `model.unregister` requires an explicit grant: classified exactly like
/// [`infer_failure_state`], with removal recovery text. A refusal the daemon
/// itself explains — unregistering the model it currently holds — arrives as
/// unavailable and keeps the daemon's own recovery line.
fn model_unregister_failure_state<T>(failure: Failure) -> ObservatoryState<T> {
    grant_failure_state(failure, MODEL_UNREGISTER_BLOCKED_RECOVERY)
}

/// `model.load` requires an explicit grant: classified exactly like
/// [`infer_failure_state`], with load recovery text. A refusal the daemon
/// itself explains — a load already running, an unregistered model, weights
/// that no longer map — arrives as unavailable and keeps the daemon's own
/// recovery line.
fn model_load_failure_state<T>(failure: Failure) -> ObservatoryState<T> {
    grant_failure_state(failure, MODEL_LOAD_BLOCKED_RECOVERY)
}

/// `model.unload` requires an explicit grant: classified exactly like
/// [`infer_failure_state`], with unload recovery text.
fn model_unload_failure_state<T>(failure: Failure) -> ObservatoryState<T> {
    grant_failure_state(failure, MODEL_UNLOAD_BLOCKED_RECOVERY)
}

/// `model.verify` requires an explicit grant: classified exactly like
/// [`infer_failure_state`], with verification recovery text.
fn model_verify_failure_state<T>(failure: Failure) -> ObservatoryState<T> {
    grant_failure_state(failure, MODEL_VERIFY_BLOCKED_RECOVERY)
}

/// `model.sweep` requires an explicit grant: classified exactly like
/// [`infer_failure_state`], with sweep recovery text.
fn model_sweep_failure_state<T>(failure: Failure) -> ObservatoryState<T> {
    grant_failure_state(failure, MODEL_SWEEP_BLOCKED_RECOVERY)
}

/// `model.delete-weights` requires an explicit grant: classified exactly like
/// [`infer_failure_state`], with deletion recovery text. The daemon's own
/// provenance refusal — Pam did not download this file — arrives as
/// unavailable and keeps the daemon's explanation and recovery line.
fn model_delete_weights_failure_state<T>(failure: Failure) -> ObservatoryState<T> {
    grant_failure_state(failure, MODEL_DELETE_WEIGHTS_BLOCKED_RECOVERY)
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

/// Every reset tier requires an explicit grant: classified exactly like
/// [`model_register_failure_state`], with reset recovery text.
fn reset_failure_state<T>(failure: Failure) -> ObservatoryState<T> {
    grant_failure_state(failure, RESET_BLOCKED_RECOVERY)
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
