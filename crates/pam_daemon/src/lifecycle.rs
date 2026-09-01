#[cfg(unix)]
use std::os::unix::fs::DirBuilderExt as _;
use std::{
    collections::HashMap,
    fmt::{self, Write as _},
    fs,
    future::Future,
    io::Write,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use cap_fs_ext::OsMetadataExt as _;
use cap_fs_ext::{DirExt as _, FollowSymlinks, OpenOptionsFollowExt as _, ambient_authority};
#[cfg(unix)]
use cap_std::fs::PermissionsExt as _;
use cap_std::fs::{Dir, OpenOptions};
#[cfg(unix)]
use nix::unistd::Uid;
use pam_connectors::{
    CancellationToken, Connector, ConnectorFailure, InvocationContext,
    aws::{
        VerifyCredentials as AwsVerifyCredentials,
        VerifyCredentialsRequest as AwsVerifyCredentialsRequest,
    },
    confluence::{
        VerifyCredentials as ConfluenceVerifyCredentials,
        VerifyCredentialsRequest as ConfluenceVerifyCredentialsRequest,
    },
    github::{VerifyCredentials, VerifyCredentialsRequest},
    jenkins::{
        VerifyCredentials as JenkinsVerifyCredentials,
        VerifyCredentialsRequest as JenkinsVerifyCredentialsRequest,
    },
    jira::{
        VerifyCredentials as JiraVerifyCredentials,
        VerifyCredentialsRequest as JiraVerifyCredentialsRequest,
    },
    sharepoint::{
        VerifyCredentials as SharePointVerifyCredentials,
        VerifyCredentialsRequest as SharePointVerifyCredentialsRequest,
    },
    sonarqube::{
        VerifyCredentials as SonarVerifyCredentials,
        VerifyCredentialsRequest as SonarVerifyCredentialsRequest,
    },
};
use pam_core::{
    APPLICATION_VERSION, ApprovalId, ContentDigest, EvidenceHandle, ProjectId, RequestId,
};
use pam_model::{
    GgufMetadata, LicenseSnapshot, ModelError, ModelKey, ModelSource, ModelsDirectorySweep,
    RegisteredModel, RuntimeError, RuntimeFinishReason, RuntimeMessage, RuntimeMessageRole,
    RuntimeRequest, RuntimeResponse, WeightsRefusal, delete_registered_weights,
    effective_models_dir, health_label, revalidate_registered_model, sweep_models_directory,
    weights_deletion_allowed, weights_refusal_message,
};
#[cfg(target_os = "macos")]
use pam_model::{MacosLlamaCppRuntime, ModelRuntime};
use pam_platform::{
    CorporateHttpClientFactory, CorporateHttpClientRequirements, IncomingRequest, LocalEndpoint,
    PacDiagnostic, ProxyBypassDiagnostic, ProxyDiagnosticStatus, ProxyEnvironmentVariable,
    ProxyInputIssueKind, ProxyRouteDiagnostic, ProxySource, ReqwestCorporateHttpClientFactory,
    SecretBackend, ServerTransport, TransportError, TransportErrorKind, diagnose_process_proxy,
    user_data_dir, user_home_dir,
};
use pam_policy::{CapabilityName, InvalidResourceName, ResourceName, redact_audit_detail};
use pam_protocol::{
    ActivityDaySummary, ActivityEventSummary, ActivityResult, ApprovalChallenge,
    ApprovalDecision as ProtocolApprovalDecision, ApprovalDecisionDisposition,
    ApprovalDecisionResult, BriefProvenance, BriefResult, CallerListResult, CallerSummary,
    CancellationDisposition, CancellationResult, Capability, CodecError, ConfigurationPresence,
    ConnectorConfigureResult, ConnectorCredentialAction, ConnectorListResult, ConnectorSummary,
    ConnectorTestDisposition, ConnectorTestResult, DaemonLifecycleResult, DaemonLogEntry,
    DaemonLogsResult, DaemonStatsResult, DanglingRegistrationSummary, Event, EventEnvelope,
    EvidenceChunk, EvidenceMetadata, EvidenceRedaction, EvidenceRetention, ExpectedTargetKind,
    Failure, FailureCode, GrantRevokeResult, LogSeverity, ModelDeleteWeightsResult,
    ModelFinishReason, ModelGenerationResult, ModelRegisterResult, ModelRegistration, ModelRole,
    ModelStatusResult, ModelSummary, ModelSweepResult, ModelUnregisterResult, ModelUsage,
    ModelVerification, ModelVerifyResult, NetworkDiagnosticsResult, OperationTruth,
    OrphanWeightsSummary, PROTOCOL_VERSION, PacState, ProjectCurrentResult,
    ProjectRequestState as ProtocolProjectRequestState,
    ProjectRequestSummary as ProtocolProjectRequestSummary, ProjectUsageSummary, ReplayResult,
    RequestEnvelope, RequestPayload, ResetResult, ResetTier, ResultBody, ResultEnvelope,
    ResultPayload, ServerMessage, SourceAvailability, StatusResult, decode_request_envelope,
    decode_server_message_envelope, encode,
};
use pam_store::{
    AcceptOutcome, AcceptRequest, ActivityDay, AppendAuditEvent,
    ApprovalDecision as StoreApprovalDecision, ApprovalDecisionOutcome, AuditEventRecord,
    AuthorizationAudit, AuthorizationOutcome, AuthorizationRequest, AuthorizeFlowRun,
    CallerAuthentication, CallerRegistration, CancelOutcome, ConnectorRecord, ConnectorTestStatus,
    EventRecord, ExpectedOperationKind, FlowAuthorizationOutcome, FlowAuthorizationRecoveryOutcome,
    LeasedRequest, ProjectCurrent as StoreProjectCurrent,
    ProjectRequestSummary as StoreProjectRequestSummary, ProjectUsage, Replay, RequestSnapshot,
    RequestState, Store, StoreError, TerminalState,
};
use sha2::{Digest as _, Sha256};
use tokio::{
    sync::{Semaphore, mpsc, oneshot},
    task::JoinSet,
};

use crate::DaemonError;
use crate::logging::{DaemonLog, LogLevel};

use crate::connectors::{
    AWS, CONFLUENCE, ConnectorRuntime, GITHUB_DEFAULT_API_BASE, JENKINS, JIRA, SHAREPOINT,
    SONARQUBE, built_in_connector_ids, is_built_in,
};
use crate::flow::{
    FLOW_OPERATION_KIND, FlowProcessing, FlowSubmissionError, PreparedFlowSubmission,
    decode_flow_transition, flow_result_truth, prepare_flow_submission, process_flow,
    verify_flow_project_root,
};
#[cfg(target_os = "macos")]
use crate::macos_admission::MacosRuntimeHostAdmission;
use crate::model_service::{ModelService, ModelServiceError, ModelWorker};
use crate::ptrack::PtrackBriefProvider;
use crate::reset::{self, CredentialStore, ResetContext, ResetError, ResetPaths};

const RESPONSE_CAPACITY: usize = 64;
const SCHEDULER_CAPACITY: usize = 64;
/// Receive failures tolerated back to back before the listener is declared
/// dead; a lone misbehaving client resets on the next healthy request.
const MAX_CONSECUTIVE_RECEIVE_FAILURES: u32 = 5;
pub(super) const FLOW_PREFLIGHT_CAPACITY: usize = 4;
const LEASE_DURATION: Duration = Duration::from_secs(3);
const LEASE_HEARTBEAT: Duration = Duration::from_secs(1);
const RECOVERY_INTERVAL: Duration = Duration::from_millis(50);
const APPROVAL_LIFETIME: Duration = Duration::from_mins(5);
const AUDIT_RETENTION: Duration = Duration::from_hours(30 * 24);
static AUDIT_EVENT_COUNTER: AtomicU64 = AtomicU64::new(0);
// UUIDs and current semantic IDs fit comfortably; this also leaves ample room for
// a maximum evidence chunk and its response envelope in the 1 MiB protocol frame.
const MAX_REQUEST_IDENTIFIER_BYTES: usize = 256;
const MAX_BRIEF_SECTION_ITEMS: usize = 16;
const MAX_BRIEF_PROVENANCE_ITEMS: usize = 32;
const MAX_BRIEF_TEXT_BYTES: usize = 4 * 1024;
const MAX_BRIEF_EVIDENCE_HANDLES: usize = 4;
const MAX_BRIEF_SOURCE_BYTES: usize = 256;
const MAX_BRIEF_DETAIL_BYTES: usize = 4 * 1024;
const DEFAULT_MODEL_DEADLINE: Duration = Duration::from_mins(5);
const MAX_MODEL_DEADLINE: Duration = Duration::from_mins(10);

#[derive(Clone)]
pub(super) struct LoadedModelService {
    key: ModelKey,
    size_bytes: u64,
    service: ModelService,
}

/// This daemon's model surface for its whole lifetime: the loaded service
/// when the requested model came up, and otherwise why it did not. Both are
/// `None` when no model was requested at all.
#[derive(Clone, Default)]
pub(super) struct ModelSurface {
    pub(super) loaded: Option<LoadedModelService>,
    pub(super) load_failure: Option<String>,
}

enum Outbound {
    Routed {
        incoming: IncomingRequest,
        messages: Vec<ServerMessage>,
        subscribe: Option<SubscriptionRequest>,
        registered: Option<oneshot::Sender<()>>,
    },
    Persisted {
        request_id: RequestId,
        messages: Vec<ServerMessage>,
        terminal: bool,
    },
    Stop {
        incoming: IncomingRequest,
        result: Box<ResultEnvelope>,
    },
}

struct SubscriptionRequest {
    canonical_request_id: RequestId,
    event_request_id: RequestId,
    observer_request_id: RequestId,
    project_id: ProjectId,
    last_sequence: u64,
}

struct Subscription {
    incoming: IncomingRequest,
    event_request_id: RequestId,
    observer_request_id: RequestId,
    project_id: ProjectId,
    last_sequence: u64,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TestStatusDispatch {
    Immediate,
    Durable,
}

/// Injectable connector credential-store override, primarily for isolated
/// tests. Its debug output never exposes the backend.
#[derive(Clone)]
pub struct ConnectorSecretOverride(pub Arc<dyn SecretBackend + Send + Sync>);

impl fmt::Debug for ConnectorSecretOverride {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ConnectorSecretOverride([REDACTED])")
    }
}

#[derive(Clone, Debug)]
pub struct DaemonConfig {
    pub endpoint: LocalEndpoint,
    pub recover: bool,
    /// Selects one registered model for the embedded runtime. No model is
    /// loaded and inference remains unavailable when this is absent.
    pub model: Option<ModelKey>,
    /// Overrides the durable `SQLite` path, primarily for isolated tests.
    pub state_path: Option<PathBuf>,
    /// Supplies planning context for read-only brief requests.
    pub brief_provider: Option<Arc<dyn BriefProvider>>,
    /// Overrides the native credential store for daemon-owned connector
    /// secrets, primarily for isolated tests. `None` uses the operating
    /// system's credential store.
    pub connector_secret_backend: Option<ConnectorSecretOverride>,
    #[cfg(test)]
    pub(crate) bypass_authentication: bool,
    #[cfg(test)]
    pub(crate) bypass_policy: bool,
    /// Bounds expensive flow authority preparation in tests.
    #[cfg(test)]
    pub(crate) flow_preflight_capacity: usize,
    /// Holds an admitted flow preflight for deterministic saturation tests.
    #[cfg(test)]
    pub(crate) flow_preflight_delay: Duration,
    /// Stands in for a multi-minute model load so tests can exercise the
    /// startup window during which clients may connect.
    #[cfg(test)]
    pub(crate) model_load_delay: Duration,
    /// Preserves durable status fixtures used to exercise scheduler recovery.
    #[cfg(test)]
    pub(crate) status_dispatch: TestStatusDispatch,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            endpoint: LocalEndpoint::default_for_user(),
            recover: false,
            model: None,
            state_path: None,
            brief_provider: None,
            connector_secret_backend: None,
            #[cfg(test)]
            bypass_authentication: false,
            #[cfg(test)]
            bypass_policy: false,
            #[cfg(test)]
            flow_preflight_capacity: FLOW_PREFLIGHT_CAPACITY,
            #[cfg(test)]
            flow_preflight_delay: Duration::ZERO,
            #[cfg(test)]
            model_load_delay: Duration::ZERO,
            #[cfg(test)]
            status_dispatch: TestStatusDispatch::Immediate,
        }
    }
}

/// Provider-neutral seam for planning-context integrations.
///
/// Providers must represent source failures and partial availability explicitly in
/// [`BriefResult::provenance`]; an unavailable source is not an empty verified one.
/// Results are bounded to 16 items per section, 32 provenance entries, 4 KiB per
/// item/detail, 256 bytes per source name, and 4 evidence handles per item.
pub trait BriefProvider: fmt::Debug + Send + Sync {
    fn brief<'a>(
        &'a self,
        project_id: &'a ProjectId,
        store: &'a Store,
    ) -> Pin<Box<dyn Future<Output = BriefResult> + Send + 'a>>;
}

#[derive(Debug)]
struct UnavailableBriefProvider;

impl BriefProvider for UnavailableBriefProvider {
    fn brief<'a>(
        &'a self,
        _project_id: &'a ProjectId,
        _store: &'a Store,
    ) -> Pin<Box<dyn Future<Output = BriefResult> + Send + 'a>> {
        Box::pin(async {
            BriefResult {
                goal: None,
                decisions: Vec::new(),
                verified: Vec::new(),
                next: Vec::new(),
                provenance: vec![BriefProvenance {
                    source: "planning-context".to_owned(),
                    availability: SourceAvailability::Unavailable,
                    truth: OperationTruth::Unresolved,
                    evidence: None,
                    detail: Some("No planning-context provider is configured.".to_owned()),
                }],
            }
        })
    }
}

/// Runs the foreground daemon until an operating-system shutdown signal arrives.
///
/// The daemon never runs itself: launching requires the single-use grant the
/// control center issues before spawning (`pam gui` → Start PAM).
///
/// # Errors
///
/// Returns [`DaemonError::LaunchNotGranted`] without a valid launch grant, and
/// otherwise when ownership, durable state, endpoint preparation, transport,
/// or protocol handling fails.
pub async fn run(recover: bool, model: Option<ModelKey>) -> Result<(), DaemonError> {
    let endpoint = LocalEndpoint::default_for_user();
    let presented = std::env::var(pam_platform::LAUNCH_GRANT_ENV).ok();
    if !pam_platform::consume_launch_grant(endpoint.runtime_dir(), presented.as_deref()) {
        return Err(DaemonError::LaunchNotGranted);
    }
    let brief_provider = std::env::current_dir()
        .ok()
        .and_then(|directory| {
            pam_platform::discover_project(&directory)
                .ok()
                .map(|project| {
                    Arc::new(PtrackBriefProvider::new(
                        project.root().to_path_buf(),
                        project.id().clone(),
                    ))
                })
        })
        .map(|provider| provider as Arc<dyn BriefProvider>);
    let config = DaemonConfig {
        endpoint,
        recover,
        model,
        brief_provider,
        ..DaemonConfig::default()
    };
    serve_until(config, async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await
}

/// Serves requests until the supplied shutdown future resolves.
///
/// # Errors
///
/// Returns [`DaemonError`] when ownership, durable state, endpoint preparation,
/// transport, or protocol handling fails.
pub async fn serve_until<F>(config: DaemonConfig, shutdown: F) -> Result<(), DaemonError>
where
    F: Future<Output = ()> + Send,
{
    serve_until_with_delay(config, shutdown, Duration::ZERO).await
}

#[allow(clippy::too_many_lines)]
pub(super) async fn serve_until_with_delay<F>(
    config: DaemonConfig,
    shutdown: F,
    processing_delay: Duration,
) -> Result<(), DaemonError>
where
    F: Future<Output = ()> + Send,
{
    let ownership = Ownership::acquire(&config.endpoint)?;
    prepare_endpoint(&config)?;
    let state_path = match &config.state_path {
        Some(path) => path.clone(),
        None => user_data_dir()?.join("state.sqlite3"),
    };
    let log_dir = state_path
        .parent()
        .map_or_else(|| PathBuf::from("logs"), |parent| parent.join("logs"));
    let log = DaemonLog::open(&log_dir);
    let reset_state_path = state_path.clone();
    let store = Store::open(state_path)?;
    store.recover_all_leases(now_ms()).await?;
    #[cfg(test)]
    let flow_preflight_capacity = config.flow_preflight_capacity;
    #[cfg(not(test))]
    let flow_preflight_capacity = FLOW_PREFLIGHT_CAPACITY;
    #[cfg(test)]
    let flow_preflight_delay = config.flow_preflight_delay;
    #[cfg(not(test))]
    let flow_preflight_delay = Duration::ZERO;
    #[cfg(test)]
    let model_load_delay = config.model_load_delay;
    #[cfg(not(test))]
    let model_load_delay = Duration::ZERO;
    #[cfg(test)]
    let durable_status = config.status_dispatch == TestStatusDispatch::Durable;
    #[cfg(not(test))]
    let durable_status = false;
    let flow_preflight_admission = Arc::new(Semaphore::new(flow_preflight_capacity));
    let brief_provider = config
        .brief_provider
        .clone()
        .unwrap_or_else(|| Arc::new(UnavailableBriefProvider));
    #[cfg(test)]
    let authentication_required = !config.bypass_authentication;
    #[cfg(not(test))]
    let authentication_required = true;
    #[cfg(test)]
    let policy_required = !config.bypass_policy;
    #[cfg(not(test))]
    let policy_required = true;
    let connectors = ConnectorRuntime::new(
        config
            .connector_secret_backend
            .clone()
            .map(|override_backend| override_backend.0),
    );
    connectors.warm(log.clone());
    // Reset resolves its root from the state database this daemon actually
    // opened, so a test daemon pointed at a scratch directory resets that
    // directory and the platform data directory is never in reach. Caller
    // credentials share the credential store connectors use, so a daemon
    // started with an injected backend purges through that same backend.
    let reset_context = ResetContext::new(
        ResetPaths::for_state_path(&reset_state_path).map_err(DaemonError::Reset)?,
        config
            .connector_secret_backend
            .clone()
            .map_or(CredentialStore::Native, |override_backend| {
                CredentialStore::Injected(override_backend.0)
            }),
    );
    // The endpoint is the daemon's only advertisement that it can serve, so it
    // is bound after the model is loaded rather than before. Loading a
    // multi-GB model is minutes of work and the control center probes health
    // throughout it; a Router socket left bound but unpolled for that long
    // keeps every abandoned probe in its fair queue, and the accept loop then
    // re-arms each dead peer's framed reader forever instead of serving. With
    // nothing bound those probes fail fast and honestly, and the ownership
    // lock taken above still keeps a second daemon out of the window.
    let (model_surface, model_worker) =
        load_model(&store, config.model.clone(), &log, model_load_delay).await?;
    let mut server = ServerTransport::bind(&config.endpoint).await?;
    let (outbound_tx, mut outbound_rx) = mpsc::channel::<Outbound>(RESPONSE_CAPACITY);
    let (scheduler_tx, scheduler_rx) = mpsc::channel::<()>(SCHEDULER_CAPACITY);
    let mut handlers = JoinSet::new();
    let mut scheduler = tokio::spawn(run_scheduler(
        store.clone(),
        scheduler_rx,
        outbound_tx.clone(),
        processing_delay,
        connectors.clone(),
        log.clone(),
    ));
    let mut subscriptions = HashMap::<RequestId, Vec<Subscription>>::new();

    let ready_message = model_surface.loaded.as_ref().map_or_else(
        || format!("PAM daemon ready (version {APPLICATION_VERSION}, protocol {PROTOCOL_VERSION})."),
        |model| {
            format!(
                "PAM daemon ready (version {APPLICATION_VERSION}, protocol {PROTOCOL_VERSION}, model {}).",
                model.key
            )
        },
    );
    println!("{ready_message}");
    log.info(ready_message);

    let _ = scheduler_tx.try_send(());
    tokio::pin!(shutdown);
    let mut scheduler_joined = false;
    let mut consecutive_receive_failures = 0u32;
    let result = loop {
        let action = tokio::select! {
            () = &mut shutdown => ServeAction::Shutdown,
            incoming = server.receive() => ServeAction::Incoming(incoming),
            outbound = outbound_rx.recv() => ServeAction::Outbound(outbound),
            completed = handlers.join_next(), if !handlers.is_empty() => {
                ServeAction::HandlerCompleted(completed)
            }
            completed = &mut scheduler, if !scheduler_joined => {
                scheduler_joined = true;
                ServeAction::SchedulerCompleted(completed)
            }
        };

        match action {
            ServeAction::Shutdown
            | ServeAction::Outbound(None)
            | ServeAction::SchedulerCompleted(Ok(Ok(()))) => break Ok(()),
            ServeAction::Incoming(Ok(incoming)) => {
                consecutive_receive_failures = 0;
                let request_store = store.clone();
                let request_outbound = outbound_tx.clone();
                let request_scheduler = scheduler_tx.clone();
                let request_brief_provider = Arc::clone(&brief_provider);
                let request_model = model_surface.clone();
                let request_flow_preflight_admission = Arc::clone(&flow_preflight_admission);
                let request_connectors = connectors.clone();
                let request_log = log.clone();
                let request_reset = reset_context.clone();
                handlers.spawn(async move {
                    handle_incoming(
                        incoming,
                        request_store,
                        request_outbound,
                        request_scheduler,
                        request_brief_provider,
                        request_model,
                        authentication_required,
                        policy_required,
                        request_flow_preflight_admission,
                        flow_preflight_delay,
                        durable_status,
                        request_connectors,
                        request_reset,
                        request_log,
                    )
                    .await
                });
            }
            ServeAction::Incoming(Err(error))
                if matches!(
                    error.kind(),
                    TransportErrorKind::InvalidMessage | TransportErrorKind::FrameTooLarge
                ) =>
            {
                log.warn(format!("rejected malformed client frame: {error}"));
            }
            ServeAction::Incoming(Err(error)) => {
                // A single misbehaving client must not stop the daemon; only a
                // persistently failing listener is fatal.
                consecutive_receive_failures += 1;
                log.error(format!(
                    "transport receive failed ({consecutive_receive_failures} in a row): {error}"
                ));
                if consecutive_receive_failures >= MAX_CONSECUTIVE_RECEIVE_FAILURES {
                    break Err(error.into());
                }
            }
            ServeAction::Outbound(Some(outbound)) => {
                let stopping = matches!(&outbound, Outbound::Stop { .. });
                match deliver_outbound(&mut server, &mut subscriptions, outbound).await {
                    Ok(false) => {}
                    Ok(true) => break Ok(()),
                    Err(error) if stopping => break Err(error),
                    Err(error) => {
                        log.error(format!("dropped undeliverable response: {error}"));
                    }
                }
            }
            ServeAction::HandlerCompleted(Some(Err(error))) => {
                if !error.is_cancelled() {
                    log.error(format!("request handler panicked: {error}"));
                }
            }
            ServeAction::SchedulerCompleted(Err(error)) => {
                break Err(DaemonError::Handler(error));
            }
            ServeAction::HandlerCompleted(Some(Ok(Err(error)))) => {
                log.error(format!("request handler failed: {error}"));
            }
            ServeAction::SchedulerCompleted(Ok(Err(error))) => break Err(error),
            ServeAction::HandlerCompleted(Some(Ok(Ok(()))) | None) => {}
        }
    };
    match &result {
        Ok(()) => log.info("PAM daemon stopping (orderly shutdown)."),
        Err(error) => log.error(format!("PAM daemon exiting on fatal error: {error}")),
    }

    handlers.abort_all();
    while handlers.join_next().await.is_some() {}
    drop(scheduler_tx);
    // The scheduler handle was consumed by `&mut scheduler` in the select
    // loop when it completed there; polling it again would panic.
    if !scheduler_joined {
        scheduler.abort();
        let _ = scheduler.await;
    }
    if let Some(worker) = model_worker {
        worker.shutdown().await;
    }
    drop(outbound_tx);
    server.close().await?;
    store.shutdown().await?;
    drop(ownership);
    result
}

/// Loads the model, holding the startup window open for tests.
async fn load_model(
    store: &Store,
    key: Option<ModelKey>,
    log: &DaemonLog,
    load_delay: Duration,
) -> Result<(ModelSurface, Option<ModelWorker>), DaemonError> {
    tokio::time::sleep(load_delay).await;
    start_model_service(store, key, log).await
}

#[cfg(target_os = "macos")]
async fn start_model_service(
    store: &Store,
    key: Option<ModelKey>,
    log: &DaemonLog,
) -> Result<(ModelSurface, Option<ModelWorker>), DaemonError> {
    let Some(key) = key else {
        return Ok((ModelSurface::default(), None));
    };
    // Store failures are misconfiguration or integrity problems and stay
    // fatal; only an actual runtime load failure degrades below.
    let registered = store.model(key.clone()).await?;
    let size_bytes = registered.size_bytes;
    let runtime = match tokio::task::spawn_blocking(move || {
        MacosLlamaCppRuntime::load(registered, Arc::new(MacosRuntimeHostAdmission))
    })
    .await
    {
        Ok(Ok(runtime)) => runtime,
        Ok(Err(error)) => return Ok(degrade_after_model_load_failure(log, &error)),
        Err(error) => return Err(DaemonError::Handler(error)),
    };
    let runtime: Arc<dyn ModelRuntime> = Arc::new(runtime);
    let (service, worker) = ModelService::start(runtime);
    Ok((
        ModelSurface {
            loaded: Some(LoadedModelService {
                key,
                size_bytes,
                service: service.clone(),
            }),
            load_failure: None,
        },
        Some(worker),
    ))
}

#[cfg(not(target_os = "macos"))]
async fn start_model_service(
    store: &Store,
    key: Option<ModelKey>,
    log: &DaemonLog,
) -> Result<(ModelSurface, Option<ModelWorker>), DaemonError> {
    let Some(key) = key else {
        return Ok((ModelSurface::default(), None));
    };
    let _ = store.model(key).await?;
    Ok(degrade_after_model_load_failure(
        log,
        &RuntimeError::InitializationFailed("embedded llama.cpp is available only on macOS"),
    ))
}

/// A model that cannot load must not stop the daemon: log the full error —
/// the GUI captures daemon stderr and the CLI prints it — and keep serving
/// without a model (inference answers `NotFound`, status reports none loaded).
///
/// The reason is also kept on the surface so `model.status` can report it for
/// as long as this daemon runs: a log line the GUI never reads back is not a
/// report.
pub(super) fn degrade_after_model_load_failure(
    log: &DaemonLog,
    error: &RuntimeError,
) -> (ModelSurface, Option<ModelWorker>) {
    let message = format!("model load failed; the daemon will serve without a model: {error}");
    eprintln!("{message}");
    log.error(message.clone());
    (
        ModelSurface {
            loaded: None,
            load_failure: Some(message),
        },
        None,
    )
}

async fn deliver_outbound(
    server: &mut ServerTransport,
    subscriptions: &mut HashMap<RequestId, Vec<Subscription>>,
    outbound: Outbound,
) -> Result<bool, DaemonError> {
    match outbound {
        Outbound::Routed {
            incoming,
            messages,
            subscribe,
            registered,
        } => {
            if send_messages(server, &incoming, &messages).await?
                && let Some(subscription) = subscribe
            {
                subscriptions
                    .entry(subscription.canonical_request_id)
                    .or_default()
                    .push(Subscription {
                        incoming,
                        event_request_id: subscription.event_request_id,
                        observer_request_id: subscription.observer_request_id,
                        project_id: subscription.project_id,
                        last_sequence: subscription.last_sequence,
                    });
            }
            if let Some(registered) = registered {
                let _ = registered.send(());
            }
        }
        Outbound::Persisted {
            request_id,
            messages,
            terminal,
        } => {
            let mut remove_request = false;
            if let Some(observers) = subscriptions.get_mut(&request_id) {
                let mut retained = Vec::with_capacity(observers.len());
                for mut observer in observers.drain(..) {
                    let filtered = messages_for_observer(&messages, &mut observer);
                    if send_messages(server, &observer.incoming, &filtered).await? && !terminal {
                        retained.push(observer);
                    }
                }
                *observers = retained;
                remove_request = observers.is_empty();
            }
            if remove_request {
                subscriptions.remove(&request_id);
            }
        }
        Outbound::Stop { incoming, result } => {
            return send_messages(server, &incoming, &[ServerMessage::Result(*result)]).await;
        }
    }
    Ok(false)
}

async fn send_messages(
    server: &mut ServerTransport,
    incoming: &IncomingRequest,
    messages: &[ServerMessage],
) -> Result<bool, DaemonError> {
    for message in messages {
        let payload = match encode(message) {
            Ok(payload) => payload,
            Err(CodecError::FrameTooLarge { .. }) => {
                let fallback = oversized_response_failure(message);
                let Ok(payload) = encode(&fallback) else {
                    return Ok(false);
                };
                if let Err(error) = server.respond(incoming, payload).await
                    && error.kind() != TransportErrorKind::ClientDisconnected
                {
                    return Err(error.into());
                }
                return Ok(false);
            }
            Err(error) => return Err(error.into()),
        };
        if let Err(error) = server.respond(incoming, payload).await {
            if error.kind() == TransportErrorKind::ClientDisconnected {
                return Ok(false);
            }
            return Err(error.into());
        }
    }
    Ok(true)
}

fn oversized_response_failure(message: &ServerMessage) -> ServerMessage {
    let (request_id, project_id) = match message {
        ServerMessage::Event(event) => (&event.request_id, &event.project_id),
        ServerMessage::Result(result) => (&result.request_id, &result.project_id),
    };
    ServerMessage::Result(ResultEnvelope {
        protocol_version: PROTOCOL_VERSION,
        request_id: request_id.clone(),
        project_id: project_id.clone(),
        body: ResultBody::Failure(Failure {
            code: FailureCode::FrameTooLarge,
            message: "response exceeded the local protocol frame limit".to_owned(),
            recovery: None,
            approval: None,
        }),
    })
}

fn messages_for_observer(
    messages: &[ServerMessage],
    observer: &mut Subscription,
) -> Vec<ServerMessage> {
    let mut filtered = Vec::new();
    for message in messages {
        match message {
            ServerMessage::Event(event) if event.sequence > observer.last_sequence => {
                observer.last_sequence = event.sequence;
                filtered.push(ServerMessage::Event(EventEnvelope {
                    protocol_version: event.protocol_version,
                    request_id: observer.event_request_id.clone(),
                    project_id: observer.project_id.clone(),
                    sequence: event.sequence,
                    event: event.event.clone(),
                }));
            }
            ServerMessage::Result(result) => {
                filtered.push(ServerMessage::Result(ResultEnvelope {
                    protocol_version: result.protocol_version,
                    request_id: observer.observer_request_id.clone(),
                    project_id: observer.project_id.clone(),
                    body: result.body.clone(),
                }));
            }
            ServerMessage::Event(_) => {}
        }
    }
    filtered
}

fn remap_messages(
    messages: Vec<ServerMessage>,
    event_request_id: &RequestId,
    observer_request_id: &RequestId,
    project_id: &ProjectId,
) -> Vec<ServerMessage> {
    messages
        .into_iter()
        .map(|message| match message {
            ServerMessage::Event(event) => ServerMessage::Event(EventEnvelope {
                protocol_version: event.protocol_version,
                request_id: event_request_id.clone(),
                project_id: project_id.clone(),
                sequence: event.sequence,
                event: event.event,
            }),
            ServerMessage::Result(result) => ServerMessage::Result(ResultEnvelope {
                protocol_version: result.protocol_version,
                request_id: observer_request_id.clone(),
                project_id: project_id.clone(),
                body: result.body,
            }),
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)]
async fn handle_incoming(
    incoming: IncomingRequest,
    store: Store,
    outbound: mpsc::Sender<Outbound>,
    scheduler: mpsc::Sender<()>,
    brief_provider: Arc<dyn BriefProvider>,
    model: ModelSurface,
    authentication_required: bool,
    policy_required: bool,
    flow_preflight_admission: Arc<Semaphore>,
    flow_preflight_delay: Duration,
    durable_status: bool,
    connectors: ConnectorRuntime,
    reset_context: ResetContext,
    log: DaemonLog,
) -> Result<(), DaemonError> {
    let Ok(request) = decode_request_envelope(incoming.payload()) else {
        return Ok(());
    };
    if !request_identifiers_are_bounded(&request) {
        send_routed(
            &outbound,
            incoming,
            vec![ServerMessage::Result(failure_result(
                &request,
                FailureCode::InvalidRequest,
                "request identifiers must contain 1 to 256 UTF-8 bytes without control characters",
            ))],
            None,
        )
        .await;
        return Ok(());
    }
    if let Some(failure) = request.unsupported_version_failure() {
        send_routed(
            &outbound,
            incoming,
            vec![ServerMessage::Result(failure)],
            None,
        )
        .await;
        return Ok(());
    }
    if request.project_id.is_daemon_scope() && !capability_is_daemon_scoped(&request.capability) {
        send_routed(
            &outbound,
            incoming,
            vec![ServerMessage::Result(failure_result(
                &request,
                FailureCode::InvalidRequest,
                "this operation needs a project; the reserved daemon scope cannot satisfy it",
            ))],
            None,
        )
        .await;
        return Ok(());
    }
    learn_project_root(&request, &store).await;
    if let (
        Capability::ApprovalDecide,
        RequestPayload::ApprovalDecide {
            approval_id,
            decision,
        },
    ) = (&request.capability, &request.payload)
    {
        let approval_resource =
            ResourceName::parse("approval").expect("static approval resource is valid");
        if let Some(failure) = request_authentication_preflight(
            &request,
            &store,
            authentication_required,
            &approval_resource,
        )
        .await?
        {
            send_routed(
                &outbound,
                incoming,
                vec![ServerMessage::Result(failure)],
                None,
            )
            .await;
            return Ok(());
        }
        handle_approval_decision(
            &request,
            approval_id.clone(),
            *decision,
            incoming,
            &store,
            &outbound,
        )
        .await;
        return Ok(());
    }
    let is_flow_run = matches!(
        (&request.capability, &request.payload),
        (Capability::FlowRun, RequestPayload::FlowRun { .. })
    );
    if is_flow_run {
        let unprepared_resource =
            ResourceName::parse("flow:unprepared").expect("static flow resource is valid");
        if let Some(failure) = request_authentication_preflight(
            &request,
            &store,
            authentication_required,
            &unprepared_resource,
        )
        .await?
        {
            send_routed(
                &outbound,
                incoming,
                vec![ServerMessage::Result(failure)],
                None,
            )
            .await;
            return Ok(());
        }
    }
    let flow_preflight_permit = if is_flow_run {
        if let Ok(permit) = Arc::clone(&flow_preflight_admission).try_acquire_owned() {
            Some(permit)
        } else {
            let mut failure = failure_result(
                &request,
                FailureCode::Busy,
                "flow preflight capacity is busy",
            );
            if let ResultBody::Failure(failure) = &mut failure.body {
                failure.recovery =
                    Some("retry the exact flow run after another preflight finishes".to_owned());
            }
            send_routed(
                &outbound,
                incoming,
                vec![ServerMessage::Result(failure)],
                None,
            )
            .await;
            return Ok(());
        }
    } else {
        None
    };
    if flow_preflight_permit.is_some() && !flow_preflight_delay.is_zero() {
        tokio::time::sleep(flow_preflight_delay).await;
    }
    let prepared_flow = match (&request.capability, &request.payload) {
        (
            Capability::FlowRun,
            RequestPayload::FlowRun {
                definition,
                project_root,
            },
        ) => {
            let verified_root =
                verify_flow_project_root(Path::new(project_root.as_str()), &request.project_id);
            match verified_root {
                Ok(root) => match prepare_flow_submission(definition, &root).await {
                    Ok(prepared) => Some(prepared),
                    Err(error) => {
                        send_routed(
                            &outbound,
                            incoming,
                            vec![ServerMessage::Result(failure_result(
                                &request,
                                FailureCode::InvalidRequest,
                                flow_submission_error_message(error),
                            ))],
                            None,
                        )
                        .await;
                        return Ok(());
                    }
                },
                Err(error) => {
                    send_routed(
                        &outbound,
                        incoming,
                        vec![ServerMessage::Result(failure_result(
                            &request,
                            FailureCode::InvalidRequest,
                            flow_submission_error_message(error),
                        ))],
                        None,
                    )
                    .await;
                    return Ok(());
                }
            }
        }
        _ => None,
    };
    drop(flow_preflight_permit);
    let flow_resource = prepared_flow
        .as_ref()
        .map(|prepared| ResourceName::parse(prepared.policy_resource.clone()))
        .transpose();
    let Ok(flow_resource) = flow_resource else {
        send_routed(
            &outbound,
            incoming,
            vec![ServerMessage::Result(failure_result(
                &request,
                FailureCode::InvalidRequest,
                "flow policy authority is invalid",
            ))],
            None,
        )
        .await;
        return Ok(());
    };
    let preflight_failure = if flow_resource.is_some() {
        None
    } else {
        request_preflight_with_resource(
            &request,
            &store,
            authentication_required,
            policy_required,
            None,
        )
        .await?
    };
    if let Some(failure) = preflight_failure {
        send_routed(
            &outbound,
            incoming,
            vec![ServerMessage::Result(failure)],
            None,
        )
        .await;
        return Ok(());
    }

    match (&request.capability, &request.payload) {
        (Capability::DaemonStatus, RequestPayload::Status) => {
            if durable_status {
                handle_durable_status(request, incoming, &store, &outbound, &scheduler).await
            } else {
                handle_status(&request, incoming, &store, &outbound).await
            }
        }
        (Capability::DaemonStop, RequestPayload::Stop) => {
            handle_stop(&request, incoming, &outbound).await;
            Ok(())
        }
        (Capability::ProjectCurrent, RequestPayload::ProjectCurrent) => {
            handle_project_current(&request, incoming, &store, &outbound).await
        }
        (Capability::DaemonActivity, RequestPayload::DaemonActivity { limit }) => {
            handle_daemon_activity(&request, *limit, incoming, &store, &outbound).await
        }
        (Capability::DaemonLogs, RequestPayload::DaemonLogs { limit }) => {
            handle_daemon_logs(&request, *limit, incoming, &log, &outbound).await;
            Ok(())
        }
        (Capability::DaemonStats, RequestPayload::DaemonStats { days }) => {
            handle_daemon_stats(&request, *days, incoming, &store, &outbound).await;
            Ok(())
        }
        (Capability::CallerList, RequestPayload::CallerList) => {
            handle_caller_list(&request, incoming, &store, &outbound).await
        }
        (Capability::ModelStatus, RequestPayload::ModelStatus) => {
            handle_model_status(&request, incoming, &store, &outbound, &model).await
        }
        (Capability::ModelRegister, RequestPayload::ModelRegister { registration }) => {
            handle_model_register(&request, registration.clone(), incoming, &store, &outbound).await
        }
        (Capability::ModelUnregister, RequestPayload::ModelUnregister { model: requested }) => {
            handle_model_unregister(
                &request,
                requested.clone(),
                incoming,
                &store,
                &outbound,
                model.loaded.as_ref(),
            )
            .await
        }
        (Capability::ModelVerify, RequestPayload::ModelVerify { model: requested }) => {
            handle_model_verify(&request, requested.clone(), incoming, &store, &outbound).await
        }
        (Capability::ModelSweep, RequestPayload::ModelSweep) => {
            handle_model_sweep(&request, incoming, &store, &outbound).await
        }
        (
            Capability::ModelDeleteWeights,
            RequestPayload::ModelDeleteWeights { model: requested },
        ) => {
            handle_model_delete_weights(
                &request,
                requested.clone(),
                incoming,
                &store,
                &outbound,
                model.loaded.as_ref(),
            )
            .await
        }
        (Capability::GrantRevoke, RequestPayload::GrantRevoke { capability }) => {
            handle_grant_revoke(&request, capability.clone(), incoming, &store, &outbound).await
        }
        (Capability::ResetAccess, RequestPayload::ResetAccess { dry_run }) => {
            handle_reset(
                &request,
                ResetTier::Access,
                *dry_run,
                incoming,
                &store,
                &outbound,
                &reset_context,
            )
            .await
        }
        (Capability::ResetIdentity, RequestPayload::ResetIdentity { dry_run }) => {
            handle_reset(
                &request,
                ResetTier::Identity,
                *dry_run,
                incoming,
                &store,
                &outbound,
                &reset_context,
            )
            .await
        }
        (Capability::ResetHistory, RequestPayload::ResetHistory { dry_run }) => {
            handle_reset(
                &request,
                ResetTier::History,
                *dry_run,
                incoming,
                &store,
                &outbound,
                &reset_context,
            )
            .await
        }
        (Capability::ResetRegistry, RequestPayload::ResetRegistry { dry_run }) => {
            handle_reset(
                &request,
                ResetTier::Registry,
                *dry_run,
                incoming,
                &store,
                &outbound,
                &reset_context,
            )
            .await
        }
        (Capability::ConnectorList, RequestPayload::ConnectorList) => {
            handle_connector_list(&request, incoming, &store, &outbound, &connectors).await
        }
        (
            Capability::ConnectorConfigure,
            RequestPayload::ConnectorConfigure {
                connector,
                enabled,
                base_url,
                credential,
            },
        ) => {
            handle_connector_configure(
                &request,
                connector.clone(),
                *enabled,
                base_url.clone(),
                credential.clone(),
                incoming,
                &store,
                &outbound,
                &connectors,
            )
            .await
        }
        (Capability::ConnectorTest, RequestPayload::ConnectorTest { connector }) => {
            handle_connector_test(
                &request,
                connector.clone(),
                incoming,
                &store,
                &outbound,
                &connectors,
            )
            .await
        }
        (Capability::FlowRun, RequestPayload::FlowRun { .. }) => {
            handle_flow_run(
                request,
                prepared_flow.expect("flow submission was prepared before preflight"),
                flow_resource.expect("flow resource was prepared before dispatch"),
                incoming,
                &store,
                &outbound,
                &scheduler,
            )
            .await
        }
        (
            Capability::CancelRequest,
            RequestPayload::Cancel {
                target_request_id,
                expected_target_kind,
            },
        ) => {
            handle_cancel(
                &request,
                target_request_id.clone(),
                *expected_target_kind,
                incoming,
                &store,
                &outbound,
                &scheduler,
            )
            .await
        }
        (
            Capability::ReplayEvents,
            RequestPayload::Replay {
                target_request_id,
                after_sequence,
                expected_target_kind,
            },
        ) => {
            handle_replay(
                &request,
                target_request_id.clone(),
                *after_sequence,
                *expected_target_kind,
                incoming,
                &store,
                &outbound,
            )
            .await
        }
        _ => {
            handle_read_only(
                &request,
                incoming,
                &store,
                &outbound,
                brief_provider.as_ref(),
                model.loaded.as_ref(),
            )
            .await
        }
    }
}

/// Capabilities that operate on the daemon itself rather than any project.
///
/// These accept the reserved [`ProjectId::daemon_scope`] identity: policy and
/// audit then record the "daemon" project, so grants recorded against it apply
/// globally. Every other capability needs a real project and rejects the
/// reserved scope before dispatch. None of these handlers read project state;
/// `daemon.status` only counts the scope's (always empty) request queue, and
/// `network.diagnostics` observes this host's TLS and proxy configuration,
/// which belongs to the daemon process rather than to any project.
/// `model.register` writes the daemon-global model registry, and
/// `grant.revoke` drops the requesting caller's own grants in the scope the
/// envelope names -- which for the GUI's Access controls is this same
/// daemon scope.
pub(super) const fn capability_is_daemon_scoped(capability: &Capability) -> bool {
    matches!(
        capability,
        Capability::DaemonStatus
            | Capability::DaemonStop
            | Capability::DaemonActivity
            | Capability::DaemonLogs
            | Capability::DaemonStats
            | Capability::CallerList
            | Capability::ModelStatus
            | Capability::ModelInfer
            | Capability::ModelRegister
            | Capability::ModelUnregister
            | Capability::ModelVerify
            | Capability::ModelSweep
            | Capability::ModelDeleteWeights
            | Capability::GrantRevoke
            | Capability::NetworkDiagnostics
            | Capability::ConnectorList
            | Capability::ConnectorConfigure
            | Capability::ConnectorTest
            // Reset clears daemon-global state, so its grants are written in
            // the daemon scope and its refusals recover with `--daemon`.
            | Capability::ResetAccess
            | Capability::ResetIdentity
            | Capability::ResetHistory
            | Capability::ResetRegistry
    )
}

const fn flow_submission_error_message(error: FlowSubmissionError) -> &'static str {
    match error {
        FlowSubmissionError::InvalidDefinition => "flow definition is invalid",
        FlowSubmissionError::UnsupportedDefinition => {
            "flow contains an action, classification, or approval unsupported by this daemon"
        }
        FlowSubmissionError::WorkspaceUnavailable => {
            "the exact workspace authority could not be established"
        }
    }
}

#[cfg(test)]
pub(super) async fn request_preflight(
    request: &RequestEnvelope,
    store: &Store,
    authentication_required: bool,
    policy_required: bool,
) -> Result<Option<ResultEnvelope>, StoreError> {
    request_preflight_with_resource(
        request,
        store,
        authentication_required,
        policy_required,
        None,
    )
    .await
}

async fn request_preflight_with_resource(
    request: &RequestEnvelope,
    store: &Store,
    authentication_required: bool,
    policy_required: bool,
    resource_override: Option<ResourceName>,
) -> Result<Option<ResultEnvelope>, StoreError> {
    if !request_shape_is_valid(request) {
        return Ok(Some(failure_result(
            request,
            FailureCode::InvalidRequest,
            "capability and payload do not match",
        )));
    }
    let resource = if let Some(resource) = resource_override {
        resource
    } else {
        let Ok(resource) = policy_resource(request) else {
            return Ok(Some(failure_result(
                request,
                FailureCode::InvalidRequest,
                "request cannot be represented as a policy resource",
            )));
        };
        resource
    };
    if authentication_required
        && !matches!(
            authenticate_request(request, store).await?,
            CallerAuthentication::Authenticated
        )
    {
        append_request_audit(
            store,
            request,
            &resource,
            "deny",
            "unauthenticated",
            "authentication failed",
        )
        .await?;
        let mut failure = failure_result(
            request,
            FailureCode::Unauthenticated,
            "caller authentication failed",
        );
        if let ResultBody::Failure(body) = &mut failure.body {
            body.recovery = Some("pam caller register".to_owned());
        }
        return Ok(Some(failure));
    }
    if policy_required {
        let outcome = authorize_request(request, resource, store).await?;
        if !matches!(outcome, AuthorizationOutcome::Allowed) {
            return Ok(Some(authorization_failure(request, outcome)));
        }
    } else if authentication_required {
        append_request_audit(
            store,
            request,
            &resource,
            "allow",
            "authenticated",
            "authentication succeeded; policy enforcement disabled",
        )
        .await?;
    }
    Ok(None)
}

async fn request_authentication_preflight(
    request: &RequestEnvelope,
    store: &Store,
    authentication_required: bool,
    resource: &ResourceName,
) -> Result<Option<ResultEnvelope>, StoreError> {
    if !request_shape_is_valid(request) {
        return Ok(Some(failure_result(
            request,
            FailureCode::InvalidRequest,
            "capability and payload do not match",
        )));
    }
    if authentication_required
        && !matches!(
            authenticate_request(request, store).await?,
            CallerAuthentication::Authenticated
        )
    {
        append_request_audit(
            store,
            request,
            resource,
            "deny",
            "unauthenticated",
            "authentication failed",
        )
        .await?;
        let mut failure = failure_result(
            request,
            FailureCode::Unauthenticated,
            "caller authentication failed",
        );
        if let ResultBody::Failure(body) = &mut failure.body {
            body.recovery = Some("pam caller register".to_owned());
        }
        return Ok(Some(failure));
    }
    Ok(None)
}

fn request_shape_is_valid(request: &RequestEnvelope) -> bool {
    let capability_matches = matches!(
        (&request.capability, &request.payload),
        (Capability::DaemonStatus, RequestPayload::Status)
            | (Capability::DaemonStop, RequestPayload::Stop)
            | (
                Capability::DaemonActivity,
                RequestPayload::DaemonActivity { .. }
            )
            | (Capability::DaemonLogs, RequestPayload::DaemonLogs { .. })
            | (Capability::DaemonStats, RequestPayload::DaemonStats { .. })
            | (Capability::CallerList, RequestPayload::CallerList)
            | (Capability::ProjectCurrent, RequestPayload::ProjectCurrent)
            | (
                Capability::ApprovalDecide,
                RequestPayload::ApprovalDecide { .. }
            )
            | (Capability::CancelRequest, RequestPayload::Cancel { .. })
            | (Capability::ReplayEvents, RequestPayload::Replay { .. })
            | (Capability::Brief, RequestPayload::Brief)
            | (
                Capability::NetworkDiagnostics,
                RequestPayload::NetworkDiagnostics
            )
            | (
                Capability::WaitForResult,
                RequestPayload::WaitForResult { .. }
            )
            | (Capability::GetResult, RequestPayload::GetResult { .. })
            | (
                Capability::InspectEvidence,
                RequestPayload::InspectEvidence { .. }
            )
            | (
                Capability::ReadEvidence,
                RequestPayload::ReadEvidence { .. }
            )
            | (Capability::ModelInfer, RequestPayload::ModelInfer { .. })
            | (Capability::ModelStatus, RequestPayload::ModelStatus)
            | (
                Capability::ModelRegister,
                RequestPayload::ModelRegister { .. }
            )
            | (
                Capability::ModelUnregister,
                RequestPayload::ModelUnregister { .. }
            )
            | (Capability::ModelVerify, RequestPayload::ModelVerify { .. })
            | (Capability::ModelSweep, RequestPayload::ModelSweep)
            | (
                Capability::ModelDeleteWeights,
                RequestPayload::ModelDeleteWeights { .. }
            )
            | (Capability::GrantRevoke, RequestPayload::GrantRevoke { .. })
            | (Capability::FlowRun, RequestPayload::FlowRun { .. })
            | (Capability::ConnectorList, RequestPayload::ConnectorList)
            | (
                Capability::ConnectorConfigure,
                RequestPayload::ConnectorConfigure { .. }
            )
            | (
                Capability::ConnectorTest,
                RequestPayload::ConnectorTest { .. }
            )
            | (Capability::ResetAccess, RequestPayload::ResetAccess { .. })
            | (
                Capability::ResetIdentity,
                RequestPayload::ResetIdentity { .. }
            )
            | (
                Capability::ResetHistory,
                RequestPayload::ResetHistory { .. }
            )
            | (
                Capability::ResetRegistry,
                RequestPayload::ResetRegistry { .. }
            )
    );
    capability_matches
        && (!matches!(request.capability, Capability::ApprovalDecide)
            || request.approval_id.is_none())
        && request.validate_model_request().is_ok()
        && request.validate_flow_request().is_ok()
        && request.validate_connector_request().is_ok()
        && request.validate_grant_request().is_ok()
}

async fn append_request_audit(
    store: &Store,
    request: &RequestEnvelope,
    resource: &ResourceName,
    decision: &str,
    outcome: &str,
    detail: &str,
) -> Result<(), StoreError> {
    let occurred_at_ms = now_ms();
    let redacted_detail = redact_audit_detail(
        format!(
            "capability={} resource={} detail={detail}",
            request.capability.policy_name(),
            resource.as_str()
        )
        .as_bytes(),
    );
    store
        .append_audit_event(AppendAuditEvent {
            event_id: request_audit_event_id(request, "authentication", occurred_at_ms),
            project_id: request.project_id.clone(),
            caller_id: request.caller_id.clone(),
            action: "request.preflight".to_owned(),
            decision: decision.to_owned(),
            outcome: outcome.to_owned(),
            redacted_detail,
            occurred_at_ms,
            retain_until_ms: occurred_at_ms
                .saturating_add(duration_ms(AUDIT_RETENTION))
                .min(i64::MAX as u64),
        })
        .await?;
    Ok(())
}

pub(super) fn request_audit_event_id(
    request: &RequestEnvelope,
    stage: &str,
    occurred_at_ms: u64,
) -> String {
    let mut hasher = Sha256::new();
    for field in [
        request.request_id.as_str(),
        request.project_id.as_str(),
        request.caller_id.as_str(),
        request.idempotency_key.as_str(),
        request.capability.policy_name(),
        stage,
    ] {
        hasher.update(u64::try_from(field.len()).unwrap_or(u64::MAX).to_le_bytes());
        hasher.update(field.as_bytes());
    }
    if let Some(approval_id) = &request.approval_id {
        hasher.update(approval_id.as_str().as_bytes());
    }
    hasher.update(occurred_at_ms.to_le_bytes());
    hasher.update(std::process::id().to_le_bytes());
    hasher.update(
        AUDIT_EVENT_COUNTER
            .fetch_add(1, Ordering::Relaxed)
            .to_le_bytes(),
    );
    hasher.update(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .to_le_bytes(),
    );
    let digest = ContentDigest::from_sha256(hasher.finalize().into());
    format!("request-preflight-{}", digest.sha256_hex())
}

async fn authenticate_request(
    request: &RequestEnvelope,
    store: &Store,
) -> Result<CallerAuthentication, StoreError> {
    let Some(credential) = request.authentication.clone() else {
        return Ok(CallerAuthentication::InvalidCredential);
    };
    store
        .authenticate_caller(request.caller_id.clone(), credential)
        .await
}

async fn authorize_request(
    request: &RequestEnvelope,
    resource: ResourceName,
    store: &Store,
) -> Result<AuthorizationOutcome, StoreError> {
    let capability = CapabilityName::parse(request.capability.policy_name())
        .expect("protocol capability names are statically valid");
    let now = now_ms();
    let detail = redact_audit_detail(
        format!(
            "capability={} resource={} detail=project policy evaluated",
            capability.as_str(),
            resource.as_str()
        )
        .as_bytes(),
    );
    store
        .authorize_audited(
            AuthorizationRequest {
                caller_id: request.caller_id.clone(),
                project_id: request.project_id.clone(),
                capability,
                resource,
                approval_id: request.approval_id.clone(),
            },
            AuthorizationAudit {
                event_id: request_audit_event_id(request, "policy", now),
                action: "request.preflight".to_owned(),
                redacted_detail: detail,
                retain_until_ms: now
                    .saturating_add(duration_ms(AUDIT_RETENTION))
                    .min(i64::MAX as u64),
            },
            now,
            duration_ms(APPROVAL_LIFETIME),
        )
        .await
}

pub(super) fn policy_resource(
    request: &RequestEnvelope,
) -> Result<ResourceName, InvalidResourceName> {
    let resource = match &request.payload {
        RequestPayload::Status
        | RequestPayload::Stop
        | RequestPayload::DaemonActivity { .. }
        | RequestPayload::DaemonLogs { .. }
        | RequestPayload::DaemonStats { .. }
        | RequestPayload::CallerList
        | RequestPayload::ModelStatus => "daemon".to_owned(),
        RequestPayload::ProjectCurrent => "project".to_owned(),
        RequestPayload::ModelRegister { registration } => format!("model:{}", registration.model),
        RequestPayload::GrantRevoke { capability } => format!("grant:{capability}"),
        RequestPayload::ConnectorList => "connector".to_owned(),
        RequestPayload::ConnectorConfigure { connector, .. }
        | RequestPayload::ConnectorTest { connector } => format!("connector:{connector}"),
        RequestPayload::ApprovalDecide { .. } => "approval".to_owned(),
        RequestPayload::Brief => format!("project:{}", request.project_id),
        RequestPayload::NetworkDiagnostics => "network:configuration".to_owned(),
        RequestPayload::Cancel {
            target_request_id,
            expected_target_kind,
        }
        | RequestPayload::GetResult {
            target_request_id,
            expected_target_kind,
        } => target_policy_resource(target_request_id, *expected_target_kind, None),
        RequestPayload::Replay {
            target_request_id,
            after_sequence,
            expected_target_kind,
        }
        | RequestPayload::WaitForResult {
            target_request_id,
            after_sequence,
            expected_target_kind,
        } => target_policy_resource(
            target_request_id,
            *expected_target_kind,
            Some(*after_sequence),
        ),
        RequestPayload::InspectEvidence { handle } => format!("evidence:{handle}"),
        RequestPayload::ReadEvidence {
            handle,
            offset,
            length,
        } => format!("evidence:{handle}:offset={offset}:length={length}"),
        // Stable across turns on purpose: a per-conversation digest made every
        // chat message a new resource, so no grant could ever match twice and
        // the denial's own recovery hint was un-followable. The cost is that a
        // `model.infer` approval binds to the model, not to one exact
        // conversation -- a chat message can never be pre-approved anyway.
        // `model.unregister` names the same authority: one exact model.
        RequestPayload::ModelInfer { model, .. }
        | RequestPayload::ModelUnregister { model }
        | RequestPayload::ModelDeleteWeights { model } => format!("model:{model}"),
        // Verification names the model it was asked about, and the whole
        // catalog when it was asked about all of them, so an approval binds to
        // exactly the scope the caller requested. `model:all` stays shell-safe
        // so the denial's own recovery command remains runnable.
        RequestPayload::ModelVerify { model } => model
            .as_ref()
            .map_or_else(|| "model:all".to_owned(), |model| format!("model:{model}")),
        // The sweep's authority is the models directory itself, not any one
        // model: it reads every row and every file under that root.
        RequestPayload::ModelSweep => "models:directory".to_owned(),
        // A dry run and a real reset are deliberately different resources: a
        // grant that only ever forecast a reset must never be spendable on
        // the wipe itself.
        RequestPayload::ResetAccess { dry_run } => reset_resource("access", *dry_run),
        RequestPayload::ResetIdentity { dry_run } => reset_resource("identity", *dry_run),
        RequestPayload::ResetHistory { dry_run } => reset_resource("history", *dry_run),
        RequestPayload::ResetRegistry { dry_run } => reset_resource("registry", *dry_run),
        // Flow execution requires an exact worktree fingerprint and is prepared by
        // `handle_incoming` before policy evaluation.
        RequestPayload::FlowRun { .. } => return Err(InvalidResourceName),
    };
    ResourceName::parse(resource)
}

fn reset_resource(tier: &str, dry_run: bool) -> String {
    let mode = if dry_run { "preview" } else { "apply" };
    format!("reset:{tier}:mode={mode}")
}

fn target_policy_resource(
    target_request_id: &RequestId,
    expected_target_kind: Option<ExpectedTargetKind>,
    after_sequence: Option<u64>,
) -> String {
    let id = target_request_id.as_str();
    if expected_target_kind.is_none() {
        return after_sequence.map_or_else(
            || format!("request:{id}"),
            |after_sequence| format!("request:{id}:after={after_sequence}"),
        );
    }
    let kind = expected_target_kind
        .expect("typed target checked above")
        .policy_label();
    let mut resource = format!("flow-request-v4:id-bytes={}:id={id}:kind={kind}", id.len());
    if let Some(after_sequence) = after_sequence {
        use std::fmt::Write as _;
        let _ = write!(resource, ":after={after_sequence}");
    }
    resource
}

fn authorization_failure(
    request: &RequestEnvelope,
    outcome: AuthorizationOutcome,
) -> ResultEnvelope {
    authorization_failure_with_resource(request, outcome, &policy_resource(request))
}

fn flow_authorization_failure(
    request: &RequestEnvelope,
    resource: &ResourceName,
    outcome: FlowAuthorizationOutcome,
) -> ResultEnvelope {
    let outcome = match outcome {
        FlowAuthorizationOutcome::Accepted(_) => {
            unreachable!("accepted flow requests are dispatched")
        }
        FlowAuthorizationOutcome::Denied => AuthorizationOutcome::Denied,
        FlowAuthorizationOutcome::ApprovalRequired {
            approval_id,
            expires_at_ms,
        } => AuthorizationOutcome::ApprovalRequired {
            approval_id,
            expires_at_ms,
        },
        FlowAuthorizationOutcome::ApprovalDenied => AuthorizationOutcome::ApprovalDenied,
        FlowAuthorizationOutcome::ApprovalExpired => AuthorizationOutcome::ApprovalExpired,
    };
    authorization_failure_with_resource(request, outcome, &Ok(resource.clone()))
}

fn authorization_failure_with_resource(
    request: &RequestEnvelope,
    outcome: AuthorizationOutcome,
    resource: &Result<ResourceName, InvalidResourceName>,
) -> ResultEnvelope {
    let (code, message, recovery, approval) = match outcome {
        AuthorizationOutcome::Allowed => unreachable!("allowed requests are dispatched"),
        AuthorizationOutcome::Denied => (
            FailureCode::Forbidden,
            "project policy denied this capability".to_owned(),
            Some(grant_recovery(request, resource)),
            None,
        ),
        AuthorizationOutcome::ApprovalRequired {
            approval_id,
            expires_at_ms,
        } => {
            let recovery = approval_recovery(request, &approval_id);
            (
                FailureCode::ApprovalRequired,
                "this exact effect requires approval".to_owned(),
                Some(recovery),
                Some(ApprovalChallenge {
                    approval_id,
                    expires_at_unix_ms: expires_at_ms,
                }),
            )
        }
        AuthorizationOutcome::ApprovalDenied => (
            FailureCode::ApprovalDenied,
            "approval was denied".to_owned(),
            None,
            None,
        ),
        AuthorizationOutcome::ApprovalExpired => (
            FailureCode::ApprovalExpired,
            "approval expired before use".to_owned(),
            None,
            None,
        ),
    };
    ResultEnvelope {
        protocol_version: PROTOCOL_VERSION,
        request_id: request.request_id.clone(),
        project_id: request.project_id.clone(),
        body: ResultBody::Failure(Failure {
            code,
            message,
            recovery,
            approval,
        }),
    }
}

pub(super) fn approval_recovery(request: &RequestEnvelope, approval_id: &ApprovalId) -> String {
    if !shell_safe_policy_argument(approval_id.as_str()) {
        return "approve the exact request using a shell-quoted approval ID, then retry without changing its effect"
            .to_owned();
    }
    if let Some(recovery) = typed_flow_approval_recovery(request, approval_id) {
        return recovery;
    }
    match &request.capability {
        Capability::DaemonStatus
        | Capability::ProjectCurrent
        | Capability::Brief
        | Capability::NetworkDiagnostics
        | Capability::WaitForResult
        | Capability::GetResult
        | Capability::ModelInfer
        | Capability::ModelUnregister
        | Capability::ModelVerify
        | Capability::ModelSweep
        | Capability::ModelDeleteWeights
        | Capability::FlowRun
        // Every reset tier has a CLI surface that takes --approval-id.
        | Capability::ResetAccess
        | Capability::ResetIdentity
        | Capability::ResetHistory
        | Capability::ResetRegistry => format!(
            "pam approval approve {approval_id}, then retry the original command with --approval-id {approval_id}"
        ),
        Capability::InspectEvidence | Capability::ReadEvidence => format!(
            "pam approval approve {approval_id}; pam evidence show spans inspection and range reads, so this one-request receipt must be retried by a protocol client against the exact challenged request"
        ),
        Capability::ApprovalDecide
        | Capability::DaemonStop
        | Capability::DaemonActivity
        | Capability::DaemonLogs
        | Capability::DaemonStats
        | Capability::CallerList
        | Capability::ModelStatus
        | Capability::ModelRegister
        | Capability::GrantRevoke
        | Capability::ConnectorList
        | Capability::ConnectorConfigure
        | Capability::ConnectorTest
        | Capability::CancelRequest
        | Capability::ReplayEvents => format!(
            "pam approval approve {approval_id}; PAM has no CLI retry surface for this capability, so a protocol client must attach this one-request receipt to the exact challenged request"
        ),
    }
}

fn typed_flow_approval_recovery(
    request: &RequestEnvelope,
    approval_id: &ApprovalId,
) -> Option<String> {
    if !shell_safe_policy_argument(approval_id.as_str()) {
        return None;
    }
    let approval = approval_id.as_str();
    let command = match &request.payload {
        RequestPayload::Cancel {
            target_request_id,
            expected_target_kind: Some(ExpectedTargetKind::FlowRun),
        } if shell_safe_policy_argument(target_request_id.as_str()) => {
            format!("pam flow cancel {target_request_id} --approval-id {approval}")
        }
        RequestPayload::Replay {
            target_request_id,
            after_sequence,
            expected_target_kind: Some(ExpectedTargetKind::FlowRun),
        } if shell_safe_policy_argument(target_request_id.as_str()) => format!(
            "pam flow logs {target_request_id} --after {after_sequence} --approval-id {approval}"
        ),
        RequestPayload::WaitForResult {
            target_request_id,
            after_sequence,
            expected_target_kind: Some(ExpectedTargetKind::FlowRun),
        } if shell_safe_policy_argument(target_request_id.as_str()) => format!(
            "pam flow wait {target_request_id} --after {after_sequence} --approval-id {approval}"
        ),
        RequestPayload::GetResult {
            target_request_id,
            expected_target_kind: Some(ExpectedTargetKind::FlowRun),
        } if shell_safe_policy_argument(target_request_id.as_str()) => {
            format!("pam flow result {target_request_id} --approval-id {approval}")
        }
        _ => return None,
    };
    Some(format!(
        "pam approval approve {approval}, then run {command}"
    ))
}

pub(super) fn grant_recovery(
    request: &RequestEnvelope,
    resource: &Result<ResourceName, InvalidResourceName>,
) -> String {
    let capability = request.capability.policy_name();
    // Grants are looked up by exact project id, so a denial on the reserved
    // daemon scope is only fixed by a grant written there.
    let scope = if request.project_id.is_daemon_scope() {
        " --daemon"
    } else {
        ""
    };
    let Ok(resource) = resource else {
        return "review the denied capability and resource before adding a grant".to_owned();
    };
    if shell_safe_policy_argument(capability) && shell_safe_policy_argument(resource.as_str()) {
        format!(
            "pam access grant {capability}{scope} --resource {}",
            resource.as_str()
        )
    } else {
        "run pam access grant with the denied capability and exact resource, quoted for your shell"
            .to_owned()
    }
}

fn shell_safe_policy_argument(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/' | b'=' | b'+')
        })
}

async fn handle_read_only(
    request: &RequestEnvelope,
    incoming: IncomingRequest,
    store: &Store,
    outbound: &mpsc::Sender<Outbound>,
    brief_provider: &dyn BriefProvider,
    loaded_model: Option<&LoadedModelService>,
) -> Result<(), DaemonError> {
    match (&request.capability, &request.payload) {
        (Capability::Brief, RequestPayload::Brief) => {
            handle_brief(request, incoming, store, outbound, brief_provider).await
        }
        (Capability::NetworkDiagnostics, RequestPayload::NetworkDiagnostics) => {
            handle_network_diagnostics(request, incoming, store, outbound).await
        }
        (Capability::ModelInfer, RequestPayload::ModelInfer { .. }) => {
            handle_model_infer(request, incoming, store, outbound, loaded_model).await
        }
        (
            Capability::WaitForResult,
            RequestPayload::WaitForResult {
                target_request_id,
                after_sequence,
                expected_target_kind,
            },
        ) => {
            handle_wait_for_result(
                request,
                target_request_id.clone(),
                *after_sequence,
                *expected_target_kind,
                incoming,
                store,
                outbound,
            )
            .await
        }
        (
            Capability::GetResult,
            RequestPayload::GetResult {
                target_request_id,
                expected_target_kind,
            },
        ) => {
            handle_get_result(
                request,
                target_request_id.clone(),
                *expected_target_kind,
                incoming,
                store,
                outbound,
            )
            .await
        }
        (Capability::InspectEvidence, RequestPayload::InspectEvidence { handle }) => {
            handle_inspect_evidence(request, handle.clone(), incoming, store, outbound).await
        }
        (
            Capability::ReadEvidence,
            RequestPayload::ReadEvidence {
                handle,
                offset,
                length,
            },
        ) => {
            handle_read_evidence(
                request,
                handle.clone(),
                *offset,
                *length,
                incoming,
                store,
                outbound,
            )
            .await
        }
        _ => {
            send_routed(
                outbound,
                incoming,
                vec![ServerMessage::Result(failure_result(
                    request,
                    FailureCode::InvalidRequest,
                    "capability and payload do not match",
                ))],
                None,
            )
            .await;
            Ok(())
        }
    }
}

#[allow(clippy::too_many_lines)]
async fn handle_model_infer(
    request: &RequestEnvelope,
    incoming: IncomingRequest,
    store: &Store,
    outbound: &mpsc::Sender<Outbound>,
    loaded: Option<&LoadedModelService>,
) -> Result<(), DaemonError> {
    let RequestPayload::ModelInfer {
        model,
        messages,
        max_output_tokens,
    } = &request.payload
    else {
        unreachable!("model inference is dispatched only for its matching payload")
    };
    let Some(loaded) = loaded.filter(|loaded| loaded.key.id() == *model) else {
        let mut failure = failure_result(
            request,
            FailureCode::NotFound,
            "the requested model is not loaded in this daemon",
        );
        if let ResultBody::Failure(body) = &mut failure.body {
            body.recovery = Some(format!("restart PAM with `pam daemon --model {model}`"));
        }
        send_routed(
            outbound,
            incoming,
            vec![ServerMessage::Result(failure)],
            None,
        )
        .await;
        return Ok(());
    };
    let deadline = match model_deadline(request.deadline_unix_ms) {
        Ok(deadline) => deadline,
        Err((code, message)) => {
            send_routed(
                outbound,
                incoming,
                vec![ServerMessage::Result(failure_result(
                    request, code, message,
                ))],
                None,
            )
            .await;
            return Ok(());
        }
    };
    let Ok(runtime_messages) = messages
        .iter()
        .map(|message| {
            RuntimeMessage::new(
                match message.role() {
                    ModelRole::System => RuntimeMessageRole::System,
                    ModelRole::User => RuntimeMessageRole::User,
                    ModelRole::Assistant => RuntimeMessageRole::Assistant,
                },
                message.content(),
            )
        })
        .collect::<Result<Vec<_>, _>>()
    else {
        send_routed(
            outbound,
            incoming,
            vec![ServerMessage::Result(failure_result(
                request,
                FailureCode::InvalidRequest,
                "model messages are invalid",
            ))],
            None,
        )
        .await;
        return Ok(());
    };
    let Ok(runtime_request) = RuntimeRequest::new(runtime_messages, *max_output_tokens) else {
        send_routed(
            outbound,
            incoming,
            vec![ServerMessage::Result(failure_result(
                request,
                FailureCode::InvalidRequest,
                "model request is invalid",
            ))],
            None,
        )
        .await;
        return Ok(());
    };
    let result = loaded.service.infer(runtime_request, deadline).await;
    let (response, audit_outcome, audit_detail) = match result {
        Ok(result) => model_runtime_result(request, model, result),
        Err(error) => {
            let (code, message, outcome) = model_service_failure(&error);
            (
                failure_result(request, code, message),
                outcome,
                format!("model={model} outcome={outcome}"),
            )
        }
    };
    append_model_audit(store, request, audit_outcome, &audit_detail).await?;
    send_routed(
        outbound,
        incoming,
        vec![ServerMessage::Result(response)],
        None,
    )
    .await;
    Ok(())
}

pub(super) fn model_runtime_result(
    request: &RequestEnvelope,
    model: &str,
    result: RuntimeResponse,
) -> (ResultEnvelope, &'static str, String) {
    let finish_reason = match result.finish_reason {
        RuntimeFinishReason::Stop => ModelFinishReason::Stop,
        RuntimeFinishReason::Length => ModelFinishReason::Length,
    };
    let usage = ModelUsage {
        input_tokens: result.usage.input_tokens,
        sampled_output_tokens: result.usage.sampled_output_tokens,
        emitted_output_tokens: result.usage.emitted_output_tokens,
    };
    match ModelGenerationResult::new(model, result.text, finish_reason, usage) {
        Ok(generation) => (
            success_result(
                request,
                OperationTruth::Observed,
                ResultPayload::ModelGeneration(generation),
            ),
            "observed",
            format!(
                "model={model} finish={} input_tokens={} sampled_output_tokens={} emitted_output_tokens={}",
                model_finish_label(finish_reason),
                usage.input_tokens,
                usage.sampled_output_tokens,
                usage.emitted_output_tokens,
            ),
        ),
        Err(_) => (
            failure_result(
                request,
                FailureCode::Internal,
                "embedded model returned an invalid result",
            ),
            "failed",
            format!("model={model} outcome=invalid_result"),
        ),
    }
}

fn model_deadline(deadline_unix_ms: Option<u64>) -> Result<Instant, (FailureCode, &'static str)> {
    let now_unix_ms = now_ms();
    let deadline_unix_ms = deadline_unix_ms
        .unwrap_or_else(|| now_unix_ms.saturating_add(duration_ms(DEFAULT_MODEL_DEADLINE)));
    if deadline_unix_ms <= now_unix_ms {
        return Err((FailureCode::Cancelled, "model request deadline has elapsed"));
    }
    let remaining_ms = deadline_unix_ms - now_unix_ms;
    if remaining_ms > duration_ms(MAX_MODEL_DEADLINE) {
        return Err((
            FailureCode::InvalidRequest,
            "model request deadline exceeds the 10 minute limit",
        ));
    }
    Instant::now()
        .checked_add(Duration::from_millis(remaining_ms))
        .ok_or((
            FailureCode::InvalidRequest,
            "model request deadline is invalid",
        ))
}

fn model_service_failure(error: &ModelServiceError) -> (FailureCode, &'static str, &'static str) {
    match error {
        ModelServiceError::Busy | ModelServiceError::Runtime(RuntimeError::Busy) => (
            FailureCode::Busy,
            "the embedded model worker is busy",
            "busy",
        ),
        ModelServiceError::DeadlineExceeded => (
            FailureCode::Cancelled,
            "model request deadline elapsed",
            "deadline_exceeded",
        ),
        ModelServiceError::Runtime(RuntimeError::Cancelled) => (
            FailureCode::Cancelled,
            "model inference was cancelled",
            "cancelled",
        ),
        ModelServiceError::Runtime(RuntimeError::InvalidRequest(_)) => (
            FailureCode::InvalidRequest,
            "model request is invalid",
            "invalid_request",
        ),
        ModelServiceError::Runtime(_) | ModelServiceError::Unavailable => (
            FailureCode::Internal,
            "embedded model inference failed",
            "failed",
        ),
    }
}

async fn append_model_audit(
    store: &Store,
    request: &RequestEnvelope,
    outcome: &str,
    detail: &str,
) -> Result<(), StoreError> {
    let occurred_at_ms = now_ms();
    store
        .append_audit_event(AppendAuditEvent {
            event_id: request_audit_event_id(request, "model-inference", occurred_at_ms),
            project_id: request.project_id.clone(),
            caller_id: request.caller_id.clone(),
            action: "model.infer".to_owned(),
            decision: "execute".to_owned(),
            outcome: outcome.to_owned(),
            redacted_detail: redact_audit_detail(detail.as_bytes()),
            occurred_at_ms,
            retain_until_ms: occurred_at_ms
                .saturating_add(duration_ms(AUDIT_RETENTION))
                .min(i64::MAX as u64),
        })
        .await?;
    Ok(())
}

const fn model_finish_label(reason: ModelFinishReason) -> &'static str {
    match reason {
        ModelFinishReason::Stop => "stop",
        ModelFinishReason::Length => "length",
    }
}

async fn handle_network_diagnostics(
    request: &RequestEnvelope,
    incoming: IncomingRequest,
    store: &Store,
    outbound: &mpsc::Sender<Outbound>,
) -> Result<(), DaemonError> {
    let (truth, result) = tokio::task::spawn_blocking(collect_network_diagnostics)
        .await
        .map_err(DaemonError::Handler)?;
    let occurred_at_ms = now_ms();
    let detail = format!(
        "platform_roots={} system_proxy={} proxy_environment={} no_proxy={} pac={}",
        result.platform_roots_enabled,
        result.system_proxy_discovery_enabled,
        configuration_presence_label(result.proxy_environment_presence),
        configuration_presence_label(result.no_proxy_presence),
        pac_state_label(result.pac_state)
    );
    store
        .append_audit_event(AppendAuditEvent {
            event_id: request_audit_event_id(request, "network-observation", occurred_at_ms),
            project_id: request.project_id.clone(),
            caller_id: request.caller_id.clone(),
            action: "network.diagnostics".to_owned(),
            decision: "observe".to_owned(),
            outcome: truth_label(&truth).to_owned(),
            redacted_detail: redact_audit_detail(detail.as_bytes()),
            occurred_at_ms,
            retain_until_ms: occurred_at_ms
                .saturating_add(duration_ms(AUDIT_RETENTION))
                .min(i64::MAX as u64),
        })
        .await?;
    send_routed(
        outbound,
        incoming,
        vec![ServerMessage::Result(success_result(
            request,
            truth,
            ResultPayload::NetworkDiagnostics(result),
        ))],
        None,
    )
    .await;
    Ok(())
}

const fn configuration_presence_label(value: ConfigurationPresence) -> &'static str {
    match value {
        ConfigurationPresence::Configured => "configured",
        ConfigurationPresence::NotConfigured => "not_configured",
        ConfigurationPresence::Invalid => "invalid",
    }
}

const fn pac_state_label(value: PacState) -> &'static str {
    match value {
        PacState::NotDetected => "not_detected",
        PacState::DetectedUnsupported => "detected_unsupported",
        PacState::InspectionUnavailable => "inspection_unavailable",
    }
}

const fn truth_label(value: &OperationTruth) -> &'static str {
    match value {
        OperationTruth::Observed => "observed",
        OperationTruth::Changed => "changed",
        OperationTruth::Verified => "verified",
        OperationTruth::Unresolved => "unresolved",
        OperationTruth::Blocked => "blocked",
    }
}

fn collect_network_diagnostics() -> (OperationTruth, NetworkDiagnosticsResult) {
    let proxy = diagnose_process_proxy();
    let client_ready = ReqwestCorporateHttpClientFactory
        .build(CorporateHttpClientRequirements::secure_default())
        .is_ok();
    let proxy_environment_presence = proxy_environment_presence(&proxy);
    let no_proxy_presence = no_proxy_presence(&proxy);
    let truth = if client_ready && proxy.status == ProxyDiagnosticStatus::Observed {
        OperationTruth::Observed
    } else {
        OperationTruth::Unresolved
    };
    let result = NetworkDiagnosticsResult {
        platform_roots_enabled: client_ready,
        system_proxy_discovery_enabled: client_ready,
        proxy_environment_presence,
        no_proxy_presence,
        pac_state: match proxy.pac {
            PacDiagnostic::DetectedButUnsupported => PacState::DetectedUnsupported,
            PacDiagnostic::NotDetected => PacState::NotDetected,
            PacDiagnostic::InspectionUnavailable(_) => PacState::InspectionUnavailable,
        },
    };
    (truth, result)
}

fn proxy_environment_presence(proxy: &pam_platform::ProxyDiagnostic) -> ConfigurationPresence {
    let environment_route = |route| {
        matches!(
            route,
            ProxyRouteDiagnostic::Configured {
                source: ProxySource::Environment(_),
                ..
            }
        )
    };
    if environment_route(proxy.http) || environment_route(proxy.https) {
        ConfigurationPresence::Configured
    } else if proxy.ignored_inputs.iter().any(|issue| {
        issue.kind == ProxyInputIssueKind::Malformed
            && matches!(
                issue.variable,
                ProxyEnvironmentVariable::HttpProxyUpper
                    | ProxyEnvironmentVariable::HttpProxyLower
                    | ProxyEnvironmentVariable::HttpsProxyUpper
                    | ProxyEnvironmentVariable::HttpsProxyLower
                    | ProxyEnvironmentVariable::AllProxyUpper
                    | ProxyEnvironmentVariable::AllProxyLower
            )
    }) {
        ConfigurationPresence::Invalid
    } else {
        ConfigurationPresence::NotConfigured
    }
}

const fn no_proxy_presence(proxy: &pam_platform::ProxyDiagnostic) -> ConfigurationPresence {
    match proxy.bypass {
        ProxyBypassDiagnostic::Configured {
            source: ProxySource::Environment(_),
        } => ConfigurationPresence::Configured,
        ProxyBypassDiagnostic::Malformed {
            source: ProxySource::Environment(_),
        } => ConfigurationPresence::Invalid,
        ProxyBypassDiagnostic::NotConfigured
        | ProxyBypassDiagnostic::Configured {
            source: ProxySource::System,
        }
        | ProxyBypassDiagnostic::Malformed {
            source: ProxySource::System,
        }
        | ProxyBypassDiagnostic::SuppressedByCgi
        | ProxyBypassDiagnostic::Unresolved(_) => ConfigurationPresence::NotConfigured,
    }
}

async fn handle_project_current(
    request: &RequestEnvelope,
    incoming: IncomingRequest,
    store: &Store,
    outbound: &mpsc::Sender<Outbound>,
) -> Result<(), DaemonError> {
    let current = match store.project_current(request.project_id.clone()).await {
        Ok(current) => current,
        Err(error) => {
            send_store_failure(outbound, incoming, request, &error).await;
            return Ok(());
        }
    };
    let Ok(result) = protocol_project_current(current) else {
        send_routed(
            outbound,
            incoming,
            vec![ServerMessage::Result(failure_result(
                request,
                FailureCode::FrameTooLarge,
                "project current metadata exceeded bounded protocol limits",
            ))],
            None,
        )
        .await;
        return Ok(());
    };
    send_routed(
        outbound,
        incoming,
        vec![ServerMessage::Result(success_result(
            request,
            OperationTruth::Observed,
            ResultPayload::ProjectCurrent(result),
        ))],
        None,
    )
    .await;
    Ok(())
}

pub(super) fn protocol_project_current(
    current: StoreProjectCurrent,
) -> Result<ProjectCurrentResult, ()> {
    let queued = current
        .queued
        .into_iter()
        .map(protocol_project_request)
        .collect::<Result<Vec<_>, _>>()?;
    let active = current.active.map(protocol_project_request).transpose()?;
    let latest = current
        .latest_terminal
        .map(protocol_project_request)
        .transpose()?;
    ProjectCurrentResult::new(queued, active, latest, current.queued_truncated).map_err(|_| ())
}

fn protocol_project_request(
    request: StoreProjectRequestSummary,
) -> Result<ProtocolProjectRequestSummary, ()> {
    ProtocolProjectRequestSummary::new(
        request.request_id,
        request.operation_kind,
        protocol_project_request_state(request.state),
        request.queue_sequence,
        request.accepted_at_ms,
        request.completed_at_ms,
    )
    .map_err(|_| ())
}

const fn protocol_project_request_state(state: RequestState) -> ProtocolProjectRequestState {
    match state {
        RequestState::Queued => ProtocolProjectRequestState::Queued,
        RequestState::Leased => ProtocolProjectRequestState::Leased,
        RequestState::CancellationRequested => ProtocolProjectRequestState::CancellationRequested,
        RequestState::Succeeded => ProtocolProjectRequestState::Succeeded,
        RequestState::Failed => ProtocolProjectRequestState::Failed,
        RequestState::Cancelled => ProtocolProjectRequestState::Cancelled,
    }
}

const DEFAULT_ACTIVITY_LIMIT: u32 = 50;
const MAX_ACTIVITY_LIMIT: u32 = 100;
const DEFAULT_LOGS_LIMIT: usize = 200;
const MAX_LOGS_LIMIT: u32 = 512;
const DEFAULT_STATS_DAYS: u64 = 182;
const MAX_STATS_DAYS: u32 = 366;

/// Clamps a requested activity feed limit to 1..=100; zero selects the default.
pub(super) const fn clamp_activity_limit(limit: u32) -> u32 {
    if limit == 0 {
        DEFAULT_ACTIVITY_LIMIT
    } else if limit > MAX_ACTIVITY_LIMIT {
        MAX_ACTIVITY_LIMIT
    } else {
        limit
    }
}

/// Clamps a requested log slice to 1..=512; zero selects the default.
const fn clamp_logs_limit(limit: u32) -> usize {
    if limit == 0 {
        DEFAULT_LOGS_LIMIT
    } else if limit > MAX_LOGS_LIMIT {
        MAX_LOGS_LIMIT as usize
    } else {
        limit as usize
    }
}

/// Clamps a requested stats window to 1..=366 days; zero selects the default.
const fn clamp_stats_days(days: u32) -> u64 {
    if days == 0 {
        DEFAULT_STATS_DAYS
    } else if days > MAX_STATS_DAYS {
        MAX_STATS_DAYS as u64
    } else {
        days as u64
    }
}

async fn handle_daemon_stats(
    request: &RequestEnvelope,
    days: u32,
    incoming: IncomingRequest,
    store: &Store,
    outbound: &mpsc::Sender<Outbound>,
) {
    const DAY_MS: u64 = 86_400_000;
    let window = clamp_stats_days(days).saturating_mul(DAY_MS);
    let since = now_ms().saturating_sub(window);
    let since = since - since % DAY_MS;
    let observed = match store.activity_days(since).await {
        Ok(observed) => observed,
        Err(error) => {
            send_store_failure(outbound, incoming, request, &error).await;
            return;
        }
    };
    let projects = match store.project_usage(since).await {
        Ok(projects) => projects,
        Err(error) => {
            send_store_failure(outbound, incoming, request, &error).await;
            return;
        }
    };
    let result = DaemonStatsResult {
        days: observed.into_iter().map(activity_day_summary).collect(),
        projects: projects.into_iter().map(project_usage_summary).collect(),
    };
    send_routed(
        outbound,
        incoming,
        vec![ServerMessage::Result(success_result(
            request,
            OperationTruth::Observed,
            ResultPayload::DaemonStats(result),
        ))],
        None,
    )
    .await;
}

const fn activity_day_summary(day: ActivityDay) -> ActivityDaySummary {
    ActivityDaySummary {
        day_start_ms: day.day_start_ms,
        events: day.events,
    }
}

fn project_usage_summary(project: ProjectUsage) -> ProjectUsageSummary {
    ProjectUsageSummary {
        project_id: project.project_id,
        events: project.events,
        last_event_ms: project.last_event_ms,
        root: project.root,
    }
}

async fn handle_daemon_logs(
    request: &RequestEnvelope,
    limit: u32,
    incoming: IncomingRequest,
    log: &DaemonLog,
    outbound: &mpsc::Sender<Outbound>,
) {
    let entries = log
        .recent(clamp_logs_limit(limit))
        .into_iter()
        .map(|entry| DaemonLogEntry {
            timestamp_ms: entry.timestamp_ms,
            severity: match entry.level {
                LogLevel::Info => LogSeverity::Info,
                LogLevel::Warn => LogSeverity::Warn,
                LogLevel::Error => LogSeverity::Error,
            },
            message: entry.message,
        })
        .collect();
    send_routed(
        outbound,
        incoming,
        vec![ServerMessage::Result(success_result(
            request,
            OperationTruth::Observed,
            ResultPayload::DaemonLogs(DaemonLogsResult { entries }),
        ))],
        None,
    )
    .await;
}

async fn handle_daemon_activity(
    request: &RequestEnvelope,
    limit: u32,
    incoming: IncomingRequest,
    store: &Store,
    outbound: &mpsc::Sender<Outbound>,
) -> Result<(), DaemonError> {
    let recent = match store.recent_audit_events(clamp_activity_limit(limit)).await {
        Ok(recent) => recent,
        Err(error) => {
            send_store_failure(outbound, incoming, request, &error).await;
            return Ok(());
        }
    };
    let result = ActivityResult {
        events: recent
            .events
            .into_iter()
            .map(protocol_activity_event)
            .collect(),
        truncated: recent.truncated,
    };
    send_routed(
        outbound,
        incoming,
        vec![ServerMessage::Result(success_result(
            request,
            OperationTruth::Observed,
            ResultPayload::DaemonActivity(result),
        ))],
        None,
    )
    .await;
    Ok(())
}

pub(super) fn protocol_activity_event(record: AuditEventRecord) -> ActivityEventSummary {
    ActivityEventSummary {
        sequence: record.sequence,
        project_id: record.project_id,
        caller_id: record.caller_id,
        action: record.action,
        decision: record.decision,
        outcome: record.outcome,
        occurred_at_ms: record.occurred_at_ms,
        project_root: record.project_root,
    }
}

async fn handle_caller_list(
    request: &RequestEnvelope,
    incoming: IncomingRequest,
    store: &Store,
    outbound: &mpsc::Sender<Outbound>,
) -> Result<(), DaemonError> {
    let callers = match store.list_callers().await {
        Ok(callers) => callers,
        Err(error) => {
            send_store_failure(outbound, incoming, request, &error).await;
            return Ok(());
        }
    };
    let result = CallerListResult {
        callers: callers.into_iter().map(protocol_caller_summary).collect(),
    };
    send_routed(
        outbound,
        incoming,
        vec![ServerMessage::Result(success_result(
            request,
            OperationTruth::Observed,
            ResultPayload::CallerList(result),
        ))],
        None,
    )
    .await;
    Ok(())
}

async fn handle_model_status(
    request: &RequestEnvelope,
    incoming: IncomingRequest,
    store: &Store,
    outbound: &mpsc::Sender<Outbound>,
    model: &ModelSurface,
) -> Result<(), DaemonError> {
    let catalog = match store.list_models().await {
        Ok(catalog) => catalog,
        Err(error) => {
            send_store_failure(outbound, incoming, request, &error).await;
            return Ok(());
        }
    };
    let registered = catalog
        .iter()
        .map(|model| ModelSummary::new(model.key.id(), model.size_bytes))
        .collect::<Result<Vec<_>, _>>();
    let message = match registered.and_then(|registered| {
        model_status_result(
            model.loaded.as_ref().map(|it| (&it.key, it.size_bytes)),
            registered,
            model.load_failure.clone(),
        )
    }) {
        Ok(result) => ServerMessage::Result(success_result(
            request,
            OperationTruth::Observed,
            ResultPayload::ModelStatus(result),
        )),
        Err(_) => ServerMessage::Result(failure_result(
            request,
            FailureCode::Internal,
            "a known model identity cannot be represented in the status contract",
        )),
    };
    send_routed(outbound, incoming, vec![message], None).await;
    Ok(())
}

const CONNECTOR_TEST_DEADLINE: Duration = Duration::from_secs(10);
const MAX_CONNECTOR_TEST_DETAIL_BYTES: usize = 1024;

fn connector_summary(
    connector_id: &str,
    record: Option<&ConnectorRecord>,
    credential_present: bool,
) -> ConnectorSummary {
    ConnectorSummary {
        connector_id: connector_id.to_owned(),
        enabled: record.is_some_and(|record| record.enabled),
        base_url: record.and_then(|record| record.base_url.clone()),
        credential_present,
        last_test_status: record
            .and_then(|record| record.last_test_status)
            .map(|status| status.as_str().to_owned()),
        last_test_at_ms: record.and_then(|record| record.last_test_at_ms),
    }
}

async fn append_change_audit(
    store: &Store,
    request: &RequestEnvelope,
    action: &str,
    outcome: &str,
    detail: &str,
) -> Result<(), StoreError> {
    let occurred_at_ms = now_ms();
    store
        .append_audit_event(AppendAuditEvent {
            event_id: request_audit_event_id(request, action, occurred_at_ms),
            project_id: request.project_id.clone(),
            caller_id: request.caller_id.clone(),
            action: action.to_owned(),
            decision: "allow".to_owned(),
            outcome: outcome.to_owned(),
            redacted_detail: redact_audit_detail(detail.as_bytes()),
            occurred_at_ms,
            retain_until_ms: occurred_at_ms
                .saturating_add(duration_ms(AUDIT_RETENTION))
                .min(i64::MAX as u64),
        })
        .await?;
    Ok(())
}

/// Registers one verified model in the durable registry the daemon owns.
///
/// The wire payload carries bounded text only: every field is rebuilt through
/// `pam_model`'s validating constructors here, so a client cannot register a
/// record the domain itself would reject.
async fn handle_model_register(
    request: &RequestEnvelope,
    registration: ModelRegistration,
    incoming: IncomingRequest,
    store: &Store,
    outbound: &mpsc::Sender<Outbound>,
) -> Result<(), DaemonError> {
    let Some(model) = registered_model(&registration) else {
        send_routed(
            outbound,
            incoming,
            vec![ServerMessage::Result(failure_result(
                request,
                FailureCode::InvalidRequest,
                "model registration is not a valid registry record",
            ))],
            None,
        )
        .await;
        return Ok(());
    };
    let key = model.key.id();
    let record = match store.put_model(model).await {
        Ok(record) => record,
        Err(error) => {
            send_store_failure(outbound, incoming, request, &error).await;
            return Ok(());
        }
    };
    append_change_audit(
        store,
        request,
        "model.register",
        "registered",
        &format!(
            "model={key} size_bytes={} digest={}",
            record.size_bytes,
            record.digest.as_str()
        ),
    )
    .await?;
    send_routed(
        outbound,
        incoming,
        vec![ServerMessage::Result(success_result(
            request,
            OperationTruth::Changed,
            ResultPayload::ModelRegister(ModelRegisterResult {
                model: record.key.id(),
                registered_at_ms: record.registered_at_ms,
            }),
        ))],
        None,
    )
    .await;
    Ok(())
}

/// Message for refusing to unregister the model this daemon currently holds.
pub(super) const MODEL_UNREGISTER_LOADED_MESSAGE: &str =
    "the requested model is loaded in this daemon and cannot be unregistered";

/// The refusal for unregistering the model this daemon currently holds, or
/// `None` when the request names any other model.
///
/// A serving daemon maps its model at startup and has no way to release it, so
/// dropping the registration under a live mapping would leave the runtime
/// serving an artifact the registry no longer knows. The recovery names the
/// only route available today: a restart without that model. Task #238 adds
/// `model.unload`; when it lands this line points at that command instead of a
/// restart.
pub(super) fn model_unregister_loaded_refusal(
    request: &RequestEnvelope,
    model: &str,
    requested: &ModelKey,
    loaded: Option<&ModelKey>,
) -> Option<ResultEnvelope> {
    if loaded != Some(requested) {
        return None;
    }
    let mut failure = failure_result(
        request,
        FailureCode::LeaseConflict,
        MODEL_UNREGISTER_LOADED_MESSAGE,
    );
    if let ResultBody::Failure(body) = &mut failure.body {
        body.recovery = Some(format!(
            "restart PAM without this model using `pam daemon`, then unregister {model}"
        ));
    }
    Some(failure)
}

/// Removes one model's registration from the durable registry the daemon owns.
///
/// The weights are never touched. `pam model import` verifies a GGUF where its
/// owner already keeps it, so the file on disk is usually not PAM's to delete;
/// removing bytes is a separate, explicit operation.
async fn handle_model_unregister(
    request: &RequestEnvelope,
    model: String,
    incoming: IncomingRequest,
    store: &Store,
    outbound: &mpsc::Sender<Outbound>,
    loaded: Option<&LoadedModelService>,
) -> Result<(), DaemonError> {
    let Some(key) = model
        .split_once('/')
        .and_then(|(vendor, name)| ModelKey::new(vendor, name).ok())
    else {
        send_routed(
            outbound,
            incoming,
            vec![ServerMessage::Result(failure_result(
                request,
                FailureCode::InvalidRequest,
                "model identity is not a valid registry identity",
            ))],
            None,
        )
        .await;
        return Ok(());
    };
    // Any other registered model unregisters normally while one is loaded.
    if let Some(failure) =
        model_unregister_loaded_refusal(request, &model, &key, loaded.map(|loaded| &loaded.key))
    {
        send_routed(
            outbound,
            incoming,
            vec![ServerMessage::Result(failure)],
            None,
        )
        .await;
        return Ok(());
    }
    let record = match store.delete_model(key).await {
        Ok(record) => record,
        Err(StoreError::ModelNotFound(model_id)) => {
            send_routed(
                outbound,
                incoming,
                vec![ServerMessage::Result(failure_result(
                    request,
                    FailureCode::NotFound,
                    &format!("model {model_id} is not registered"),
                ))],
                None,
            )
            .await;
            return Ok(());
        }
        Err(error) => {
            send_store_failure(outbound, incoming, request, &error).await;
            return Ok(());
        }
    };
    let removed = record.key.id();
    append_change_audit(
        store,
        request,
        "model.unregister",
        "unregistered",
        &format!(
            "model={removed} size_bytes={} digest={}",
            record.size_bytes,
            record.digest.as_str()
        ),
    )
    .await?;
    send_routed(
        outbound,
        incoming,
        vec![ServerMessage::Result(success_result(
            request,
            OperationTruth::Changed,
            ResultPayload::ModelUnregister(ModelUnregisterResult {
                model: removed,
                size_bytes: record.size_bytes,
                digest: record.digest.as_str().to_owned(),
            }),
        ))],
        None,
    )
    .await;
    Ok(())
}

/// The models directory this daemon reconciles the registry against.
///
/// Resolved from the same Settings-persisted preference the GUI reads, never
/// assembled from literals here: the directory a sweep walks and the root a
/// weights deletion is confined to must be the one directory the user
/// actually configured.
fn daemon_models_dir() -> Option<PathBuf> {
    let data_dir = user_data_dir().ok()?;
    let home = user_home_dir().ok()?;
    Some(effective_models_dir(&data_dir, &home))
}

/// Message for a request the daemon cannot answer without a models directory.
pub(super) const MODELS_DIR_UNRESOLVED_MESSAGE: &str =
    "PAM could not resolve the models directory to reconcile against";

fn models_dir_unresolved(request: &RequestEnvelope) -> ResultEnvelope {
    let mut failure = failure_result(
        request,
        FailureCode::Internal,
        MODELS_DIR_UNRESOLVED_MESSAGE,
    );
    if let ResultBody::Failure(body) = &mut failure.body {
        body.recovery = Some(
            "Verify the operating system user profile and PAM's Settings, then retry.".to_owned(),
        );
    }
    failure
}

/// Builds one model's verification line from the revalidation outcome.
///
/// A failure never collapses to a boolean: `health` names which check stopped
/// matching and `detail` carries that check's own sentence.
pub(super) fn model_verification(
    model: &RegisteredModel,
    outcome: &Result<(), ModelError>,
    deletable: bool,
) -> ModelVerification {
    let (health, detail) = match outcome {
        Ok(()) => ("ok", None),
        Err(error) => (health_label(error), Some(error.to_string())),
    };
    ModelVerification {
        model: model.key.id(),
        path: model.path.display().to_string(),
        size_bytes: model.size_bytes,
        health: health.to_owned(),
        detail,
        source: model.source.kind().to_owned(),
        weights_deletable: deletable,
    }
}

/// The truth a verification pass reports.
///
/// A pass that re-read every artifact and found them all intact is the one
/// thing PAM can honestly call `Verified` — the same standing the connector
/// self-test earns by actually reaching its host. Any artifact that no longer
/// matches its registration leaves the catalog `Unresolved`: PAM knows the
/// registry is wrong but cannot say what the truth is. Verifying an empty
/// catalog verified nothing, so it is a plain `Observed` read.
pub(super) fn model_verify_truth(models: &[ModelVerification]) -> OperationTruth {
    if models.is_empty() {
        OperationTruth::Observed
    } else if models.iter().all(|model| model.health == "ok") {
        OperationTruth::Verified
    } else {
        OperationTruth::Unresolved
    }
}

/// Re-reads the registered weights and reports what still matches the
/// registry.
///
/// This is the standalone form of the check the macOS runtime already runs
/// before it maps a model. Hashing a multi-gigabyte artifact is blocking work,
/// so the whole pass runs on a blocking task rather than on the request loop.
/// The registry rows one verification request covers: the named model, or the
/// whole catalog when none was named.
///
/// Returns the failure envelope to send instead when the identity is not a
/// registry identity, the model is not registered, or the store is unavailable.
async fn verification_catalog(
    request: &RequestEnvelope,
    model: Option<String>,
    store: &Store,
) -> Result<Vec<RegisteredModel>, ResultEnvelope> {
    let Some(model) = model else {
        return store
            .list_models()
            .await
            .map_err(|error| store_failure_result(request, &error));
    };
    let key = model
        .split_once('/')
        .and_then(|(vendor, name)| ModelKey::new(vendor, name).ok())
        .ok_or_else(|| {
            failure_result(
                request,
                FailureCode::InvalidRequest,
                "model identity is not a valid registry identity",
            )
        })?;
    registered_record(request, key, store)
        .await
        .map(|record| vec![record])
}

/// One registered model, or the failure envelope that says why it is not
/// available: an unknown identity is a plain `NotFound`, never an internal
/// error.
async fn registered_record(
    request: &RequestEnvelope,
    key: ModelKey,
    store: &Store,
) -> Result<RegisteredModel, ResultEnvelope> {
    store.model(key).await.map_err(|error| match error {
        StoreError::ModelNotFound(model_id) => failure_result(
            request,
            FailureCode::NotFound,
            &format!("model {model_id} is not registered"),
        ),
        error => store_failure_result(request, &error),
    })
}

async fn handle_model_verify(
    request: &RequestEnvelope,
    model: Option<String>,
    incoming: IncomingRequest,
    store: &Store,
    outbound: &mpsc::Sender<Outbound>,
) -> Result<(), DaemonError> {
    let catalog = match verification_catalog(request, model, store).await {
        Ok(catalog) => catalog,
        Err(failure) => {
            send_routed(
                outbound,
                incoming,
                vec![ServerMessage::Result(failure)],
                None,
            )
            .await;
            return Ok(());
        }
    };
    let models_dir = daemon_models_dir();
    let verified = tokio::task::spawn_blocking(move || {
        catalog
            .into_iter()
            .map(|model| {
                let outcome = revalidate_registered_model(&model);
                let deletable = models_dir
                    .as_deref()
                    .is_some_and(|root| weights_deletion_allowed(root, &model).is_ok());
                model_verification(&model, &outcome, deletable)
            })
            .collect::<Vec<_>>()
    })
    .await;
    let Ok(models) = verified else {
        send_routed(
            outbound,
            incoming,
            vec![ServerMessage::Result(failure_result(
                request,
                FailureCode::Internal,
                "model verification did not finish",
            ))],
            None,
        )
        .await;
        return Ok(());
    };
    let truth = model_verify_truth(&models);
    let failures = models.iter().filter(|model| model.health != "ok").count();
    append_change_audit(
        store,
        request,
        "model.verify",
        truth_label(&truth),
        &format!("models={} failed={failures}", models.len()),
    )
    .await?;
    send_routed(
        outbound,
        incoming,
        vec![ServerMessage::Result(success_result(
            request,
            truth,
            ResultPayload::ModelVerify(ModelVerifyResult { models }),
        ))],
        None,
    )
    .await;
    Ok(())
}

/// Turns one sweep of the models directory into its wire report.
pub(super) fn model_sweep_result(sweep: ModelsDirectorySweep) -> ModelSweepResult {
    ModelSweepResult {
        models_dir: sweep.models_dir.display().to_string(),
        dangling: sweep
            .dangling
            .into_iter()
            .map(|row| DanglingRegistrationSummary {
                model: row.key.id(),
                path: row.path.display().to_string(),
                size_bytes: row.size_bytes,
            })
            .collect(),
        orphans: sweep
            .orphans
            .into_iter()
            .map(|orphan| OrphanWeightsSummary {
                path: orphan.path.display().to_string(),
                size_bytes: orphan.size_bytes,
            })
            .collect(),
        total_bytes: sweep.total_bytes,
    }
}

/// Reconciles the registry against the models directory, in both directions.
///
/// The sweep reports and never acts: a dangling row is cleared with
/// `model.unregister`, and an orphaned file is removed by its owner or,
/// when PAM downloaded it, through `model.delete-weights`.
async fn handle_model_sweep(
    request: &RequestEnvelope,
    incoming: IncomingRequest,
    store: &Store,
    outbound: &mpsc::Sender<Outbound>,
) -> Result<(), DaemonError> {
    let catalog = match store.list_models().await {
        Ok(catalog) => catalog,
        Err(error) => {
            send_store_failure(outbound, incoming, request, &error).await;
            return Ok(());
        }
    };
    let Some(models_dir) = daemon_models_dir() else {
        send_routed(
            outbound,
            incoming,
            vec![ServerMessage::Result(models_dir_unresolved(request))],
            None,
        )
        .await;
        return Ok(());
    };
    let swept =
        tokio::task::spawn_blocking(move || sweep_models_directory(&models_dir, &catalog)).await;
    let Ok(sweep) = swept else {
        send_routed(
            outbound,
            incoming,
            vec![ServerMessage::Result(failure_result(
                request,
                FailureCode::Internal,
                "the models directory sweep did not finish",
            ))],
            None,
        )
        .await;
        return Ok(());
    };
    send_routed(
        outbound,
        incoming,
        vec![ServerMessage::Result(success_result(
            request,
            OperationTruth::Observed,
            ResultPayload::ModelSweep(model_sweep_result(sweep)),
        ))],
        None,
    )
    .await;
    Ok(())
}

/// Message for refusing to delete the weights of the model this daemon holds.
pub(super) const MODEL_DELETE_WEIGHTS_LOADED_MESSAGE: &str =
    "the requested model is loaded in this daemon and its weights cannot be deleted";

/// The refusal for deleting the weights the running daemon has mapped, or
/// `None` when the request names any other model.
///
/// Mirrors the `model.unregister` refusal: a serving daemon maps its artifact
/// for its whole life, and removing those bytes underneath it would leave the
/// runtime serving a file that no longer exists.
pub(super) fn model_delete_weights_loaded_refusal(
    request: &RequestEnvelope,
    model: &str,
    requested: &ModelKey,
    loaded: Option<&ModelKey>,
) -> Option<ResultEnvelope> {
    if loaded != Some(requested) {
        return None;
    }
    let mut failure = failure_result(
        request,
        FailureCode::LeaseConflict,
        MODEL_DELETE_WEIGHTS_LOADED_MESSAGE,
    );
    if let ResultBody::Failure(body) = &mut failure.body {
        body.recovery = Some(format!(
            "restart PAM without this model using `pam daemon`, then delete the weights for {model}"
        ));
    }
    Some(failure)
}

/// The exact words PAM refuses a weights deletion in, and what the user can do
/// instead.
///
/// The provenance refusal is the important one: `pam model import` verifies a
/// GGUF where its owner already keeps it, so PAM never owned that file and
/// deleting it would destroy a user's own data. The refusal says so, and
/// points at the two things the user can still do — drop the registry entry,
/// and remove the file themselves.
pub(super) fn weights_refusal_failure(
    refusal: WeightsRefusal,
    model: &str,
    path: &Path,
    models_dir: &Path,
) -> (FailureCode, String, String) {
    let path = path.display();
    let message = format!("{} at {path}", weights_refusal_message(refusal));
    let recovery = match refusal {
        WeightsRefusal::NotDownloadedByPam => format!(
            "Run `pam model unregister {model} --yes` to drop the registry entry, then delete {path} yourself."
        ),
        WeightsRefusal::OutsideModelsDirectory => format!(
            "Move the file back under {}, or run `pam model unregister {model} --yes` and delete {path} yourself.",
            models_dir.display()
        ),
        WeightsRefusal::Unsafe => format!(
            "Inspect {path}, then run `pam model sweep` to see what the registry and the models directory disagree about."
        ),
    };
    (FailureCode::InvalidRequest, message, recovery)
}

/// Deletes one PAM-downloaded model's weights and unregisters it.
///
/// Two gates stand in front of the removal, and both are the daemon's to
/// enforce: the model must not be the one this daemon has mapped, and the
/// registration must say PAM downloaded the artifact into the models
/// directory it is still sitting in. The removal itself is confined to that
/// directory and never follows a symlink out of it.
///
/// The bytes go before the row. A store failure after the file is gone leaves
/// exactly the dangling registration `model.sweep` reports and
/// `model.unregister` clears; losing the row first and then failing to delete
/// would instead leave an orphan nothing points at.
/// The refusal envelope for a weights deletion the gate turned down, with the
/// explanation and the recovery attached.
fn weights_refusal_result(
    request: &RequestEnvelope,
    refusal: WeightsRefusal,
    model: &str,
    path: &Path,
    models_dir: &Path,
) -> ResultEnvelope {
    let (code, message, recovery) = weights_refusal_failure(refusal, model, path, models_dir);
    let mut failure = failure_result(request, code, &message);
    if let ResultBody::Failure(body) = &mut failure.body {
        body.recovery = Some(recovery);
    }
    failure
}

#[allow(clippy::too_many_lines)] // One refusal per gate keeps the deletion path readable.
async fn handle_model_delete_weights(
    request: &RequestEnvelope,
    model: String,
    incoming: IncomingRequest,
    store: &Store,
    outbound: &mpsc::Sender<Outbound>,
    loaded: Option<&LoadedModelService>,
) -> Result<(), DaemonError> {
    // Every branch below either refuses with one envelope or falls through to
    // the removal, so the refusal path is written once.
    let refused = |failure: ResultEnvelope| vec![ServerMessage::Result(failure)];
    let Some(key) = model
        .split_once('/')
        .and_then(|(vendor, name)| ModelKey::new(vendor, name).ok())
    else {
        send_routed(
            outbound,
            incoming,
            refused(failure_result(
                request,
                FailureCode::InvalidRequest,
                "model identity is not a valid registry identity",
            )),
            None,
        )
        .await;
        return Ok(());
    };
    if let Some(failure) =
        model_delete_weights_loaded_refusal(request, &model, &key, loaded.map(|loaded| &loaded.key))
    {
        send_routed(outbound, incoming, refused(failure), None).await;
        return Ok(());
    }
    let record = match registered_record(request, key.clone(), store).await {
        Ok(record) => record,
        Err(failure) => {
            send_routed(outbound, incoming, refused(failure), None).await;
            return Ok(());
        }
    };
    let Some(models_dir) = daemon_models_dir() else {
        send_routed(
            outbound,
            incoming,
            refused(models_dir_unresolved(request)),
            None,
        )
        .await;
        return Ok(());
    };
    let path = record.path.clone();
    let root = models_dir.clone();
    let removed =
        tokio::task::spawn_blocking(move || delete_registered_weights(&root, &record)).await;
    let bytes_reclaimed = match removed {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(refusal)) => {
            send_routed(
                outbound,
                incoming,
                refused(weights_refusal_result(
                    request,
                    refusal,
                    &model,
                    &path,
                    &models_dir,
                )),
                None,
            )
            .await;
            return Ok(());
        }
        Err(_) => {
            send_routed(
                outbound,
                incoming,
                refused(failure_result(
                    request,
                    FailureCode::Internal,
                    "the weights deletion did not finish",
                )),
                None,
            )
            .await;
            return Ok(());
        }
    };
    if let Err(error) = store.delete_model(key).await {
        send_routed(
            outbound,
            incoming,
            refused(store_failure_result(request, &error)),
            None,
        )
        .await;
        return Ok(());
    }
    append_change_audit(
        store,
        request,
        "model.delete-weights",
        "deleted",
        &format!(
            "model={model} path={} bytes_reclaimed={bytes_reclaimed}",
            path.display()
        ),
    )
    .await?;
    send_routed(
        outbound,
        incoming,
        vec![ServerMessage::Result(success_result(
            request,
            OperationTruth::Changed,
            ResultPayload::ModelDeleteWeights(ModelDeleteWeightsResult {
                model,
                path: path.display().to_string(),
                bytes_reclaimed,
            }),
        ))],
        None,
    )
    .await;
    Ok(())
}

fn registered_model(registration: &ModelRegistration) -> Option<RegisteredModel> {
    let (vendor, name) = registration.model.split_once('/')?;
    let license = LicenseSnapshot::new(
        registration.license_id.clone(),
        registration.license_url.clone(),
        ContentDigest::parse(registration.license_digest.clone()).ok()?,
    )
    .ok()?;
    let source = match registration.source_url.as_deref() {
        None => ModelSource::Local,
        Some(url) => ModelSource::https(url).ok()?,
    };
    Some(RegisteredModel {
        key: ModelKey::new(vendor, name).ok()?,
        path: PathBuf::from(&registration.path),
        digest: ContentDigest::parse(registration.digest.clone()).ok()?,
        size_bytes: registration.size_bytes,
        gguf: GgufMetadata {
            version: registration.gguf_version,
            tensor_count: registration.gguf_tensor_count,
            metadata_kv_count: registration.gguf_metadata_kv_count,
            architecture: None,
            model_name: None,
            license: None,
        },
        license,
        source,
        registered_at_ms: registration.registered_at_ms,
    })
}

/// Revokes every active grant the requesting caller holds for one capability.
///
/// The caller and project come from the authenticated envelope, never from the
/// payload: this request can only drop the requester's own authority, which is
/// what makes `grant.revoke` safe as a baseline capability.
async fn handle_grant_revoke(
    request: &RequestEnvelope,
    capability: String,
    incoming: IncomingRequest,
    store: &Store,
    outbound: &mpsc::Sender<Outbound>,
) -> Result<(), DaemonError> {
    let Ok(capability) = CapabilityName::parse(capability) else {
        send_routed(
            outbound,
            incoming,
            vec![ServerMessage::Result(failure_result(
                request,
                FailureCode::InvalidRequest,
                "capability name is not a valid policy capability",
            ))],
            None,
        )
        .await;
        return Ok(());
    };
    let now = now_ms();
    let active = match store
        .active_grants(request.caller_id.clone(), request.project_id.clone(), now)
        .await
    {
        Ok(active) => active,
        Err(error) => {
            send_store_failure(outbound, incoming, request, &error).await;
            return Ok(());
        }
    };
    let mut revoked = 0_u32;
    for grant in active.iter().filter(|grant| grant.capability == capability) {
        if let Err(error) = store.revoke_grant(grant.id.clone(), now).await {
            send_store_failure(outbound, incoming, request, &error).await;
            return Ok(());
        }
        revoked = revoked.saturating_add(1);
    }
    append_change_audit(
        store,
        request,
        "grant.revoke",
        "revoked",
        &format!("capability={} revoked={revoked}", capability.as_str()),
    )
    .await?;
    send_routed(
        outbound,
        incoming,
        vec![ServerMessage::Result(success_result(
            request,
            OperationTruth::Changed,
            ResultPayload::GrantRevoke(GrantRevokeResult {
                capability: capability.as_str().to_owned(),
                revoked,
            }),
        ))],
        None,
    )
    .await;
    Ok(())
}

/// Runs one scoped reset tier, or forecasts it exactly.
///
/// A dry run reports [`OperationTruth::Observed`] and audits as an
/// observation, because it changes nothing; a real run reports
/// [`OperationTruth::Changed`] and audits the exact counts that went. Both
/// travel the same policy gate, and both refuse with a recovery line.
async fn handle_reset(
    request: &RequestEnvelope,
    tier: ResetTier,
    dry_run: bool,
    incoming: IncomingRequest,
    store: &Store,
    outbound: &mpsc::Sender<Outbound>,
    context: &ResetContext,
) -> Result<(), DaemonError> {
    let result = match reset::run_tier(store, context, tier, dry_run).await {
        Ok(result) => result,
        Err(error) => {
            let mut failure =
                failure_result(request, reset_failure_code(&error), &error.to_string());
            if let ResultBody::Failure(body) = &mut failure.body {
                body.recovery = error.recovery();
            }
            send_routed(
                outbound,
                incoming,
                vec![ServerMessage::Result(failure)],
                None,
            )
            .await;
            return Ok(());
        }
    };
    let truth = if dry_run {
        OperationTruth::Observed
    } else {
        OperationTruth::Changed
    };
    let detail = reset_audit_detail(&result);
    let occurred_at_ms = now_ms();
    store
        .append_audit_event(AppendAuditEvent {
            event_id: request_audit_event_id(request, "reset", occurred_at_ms),
            project_id: request.project_id.clone(),
            caller_id: request.caller_id.clone(),
            action: format!("reset.{}", tier.label()),
            decision: if dry_run { "observe" } else { "allow" }.to_owned(),
            outcome: truth_label(&truth).to_owned(),
            redacted_detail: redact_audit_detail(detail.as_bytes()),
            occurred_at_ms,
            retain_until_ms: occurred_at_ms
                .saturating_add(duration_ms(AUDIT_RETENTION))
                .min(i64::MAX as u64),
        })
        .await?;
    send_routed(
        outbound,
        incoming,
        vec![ServerMessage::Result(success_result(
            request,
            truth,
            ResultPayload::Reset(result),
        ))],
        None,
    )
    .await;
    Ok(())
}

const fn reset_failure_code(error: &ResetError) -> FailureCode {
    match error {
        ResetError::OutsideRoot(_) => FailureCode::InvalidRequest,
        ResetError::DaemonRunning => FailureCode::LeaseConflict,
        _ => FailureCode::Internal,
    }
}

fn reset_audit_detail(result: &ResetResult) -> String {
    let mut detail = format!(
        "scope={} dry_run={} items={} bytes={}",
        result.scope, result.dry_run, result.total_items, result.total_bytes
    );
    for entry in &result.items {
        let _ = write!(detail, " {}={}", entry.kind, entry.count);
    }
    detail
}

async fn handle_connector_list(
    request: &RequestEnvelope,
    incoming: IncomingRequest,
    store: &Store,
    outbound: &mpsc::Sender<Outbound>,
    connectors: &ConnectorRuntime,
) -> Result<(), DaemonError> {
    let records = match store.list_connectors().await {
        Ok(records) => records,
        Err(error) => {
            send_store_failure(outbound, incoming, request, &error).await;
            return Ok(());
        }
    };
    let mut summaries = Vec::new();
    for connector_id in built_in_connector_ids() {
        let record = records
            .iter()
            .find(|record| record.connector_id == connector_id);
        // Presence is display-only: an unavailable secret backend (headless
        // Linux without Secret Service) must not fail the whole listing.
        // Configure and test still surface backend errors loudly.
        let credential_present = connectors
            .credential_present(connector_id)
            .await
            .unwrap_or(false);
        summaries.push(connector_summary(connector_id, record, credential_present));
    }
    send_routed(
        outbound,
        incoming,
        vec![ServerMessage::Result(success_result(
            request,
            OperationTruth::Observed,
            ResultPayload::ConnectorList(ConnectorListResult {
                connectors: summaries,
            }),
        ))],
        None,
    )
    .await;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)] // Credential change, persistence, and audit stay in one auditable path.
async fn handle_connector_configure(
    request: &RequestEnvelope,
    connector: String,
    enabled: Option<bool>,
    base_url: Option<String>,
    credential: Option<ConnectorCredentialAction>,
    incoming: IncomingRequest,
    store: &Store,
    outbound: &mpsc::Sender<Outbound>,
    connectors: &ConnectorRuntime,
) -> Result<(), DaemonError> {
    if !is_built_in(&connector) {
        send_routed(
            outbound,
            incoming,
            vec![ServerMessage::Result(failure_result(
                request,
                FailureCode::NotFound,
                "connector is not built into this daemon",
            ))],
            None,
        )
        .await;
        return Ok(());
    }
    // Apply the credential change first so a persisted `enabled` flag can never
    // point at a credential that failed to store.
    let credential_change = match &credential {
        Some(ConnectorCredentialAction::Set { secret }) => {
            let result = connectors
                .set_credential(&connector, secret.expose_secret().to_owned())
                .await;
            if let Err(error) = result {
                send_routed(
                    outbound,
                    incoming,
                    vec![ServerMessage::Result(failure_result(
                        request,
                        FailureCode::Internal,
                        error.message(),
                    ))],
                    None,
                )
                .await;
                return Ok(());
            }
            "set"
        }
        Some(ConnectorCredentialAction::Clear) => {
            if let Err(error) = connectors.clear_credential(&connector).await {
                send_routed(
                    outbound,
                    incoming,
                    vec![ServerMessage::Result(failure_result(
                        request,
                        FailureCode::Internal,
                        error.message(),
                    ))],
                    None,
                )
                .await;
                return Ok(());
            }
            "cleared"
        }
        None => "unchanged",
    };
    let record = match store
        .upsert_connector_config(pam_store::UpsertConnectorConfig {
            connector_id: connector.clone(),
            enabled,
            base_url: base_url.clone(),
            now_ms: now_ms(),
        })
        .await
    {
        Ok(record) => record,
        Err(error) => {
            send_store_failure(outbound, incoming, request, &error).await;
            return Ok(());
        }
    };
    // The credential value itself never reaches the audit ledger.
    append_change_audit(
        store,
        request,
        "connector.configure",
        "configured",
        &format!(
            "connector={connector} enabled={} base_url_changed={} credential={credential_change}",
            record.enabled,
            base_url.is_some(),
        ),
    )
    .await?;
    let credential_present = connectors
        .credential_present(&connector)
        .await
        .unwrap_or(false);
    send_routed(
        outbound,
        incoming,
        vec![ServerMessage::Result(success_result(
            request,
            OperationTruth::Changed,
            ResultPayload::ConnectorConfigure(ConnectorConfigureResult {
                connector: connector_summary(&connector, Some(&record), credential_present),
            }),
        ))],
        None,
    )
    .await;
    Ok(())
}

async fn handle_connector_test(
    request: &RequestEnvelope,
    connector: String,
    incoming: IncomingRequest,
    store: &Store,
    outbound: &mpsc::Sender<Outbound>,
    connectors: &ConnectorRuntime,
) -> Result<(), DaemonError> {
    if !is_built_in(&connector) {
        send_routed(
            outbound,
            incoming,
            vec![ServerMessage::Result(failure_result(
                request,
                FailureCode::NotFound,
                "connector is not built into this daemon",
            ))],
            None,
        )
        .await;
        return Ok(());
    }
    let record = match store.list_connectors().await {
        Ok(records) => records
            .into_iter()
            .find(|record| record.connector_id == connector),
        Err(error) => {
            send_store_failure(outbound, incoming, request, &error).await;
            return Ok(());
        }
    };
    let base_url = record.and_then(|record| record.base_url);
    let detail = match connectors.load_credential(&connector).await {
        Ok(Some(token)) => {
            run_connector_probe(connectors, &connector, base_url.as_deref(), Some(token)).await
        }
        // The AWS connector's stored value is only an optional profile name,
        // so its probe runs against the CLI's default credential chain.
        Ok(None) if connector == AWS => {
            run_connector_probe(connectors, &connector, base_url.as_deref(), None).await
        }
        Ok(None) => Err(
            "no credential is stored for this connector; store one with connector.configure"
                .to_owned(),
        ),
        Err(error) => Err(error.message().to_owned()),
    };
    let (status, disposition, detail) = match detail {
        Ok(detail) => (
            ConnectorTestStatus::Passed,
            ConnectorTestDisposition::Passed,
            detail,
        ),
        Err(detail) => (
            ConnectorTestStatus::Failed,
            ConnectorTestDisposition::Failed,
            detail,
        ),
    };
    let detail = bounded_connector_detail(detail);
    if let Err(error) = store
        .record_connector_test(connector.clone(), status, now_ms())
        .await
    {
        send_store_failure(outbound, incoming, request, &error).await;
        return Ok(());
    }
    append_change_audit(
        store,
        request,
        "connector.test",
        status.as_str(),
        &format!("connector={connector} status={}", status.as_str()),
    )
    .await?;
    let truth = match disposition {
        ConnectorTestDisposition::Passed => OperationTruth::Verified,
        ConnectorTestDisposition::Failed => OperationTruth::Observed,
    };
    send_routed(
        outbound,
        incoming,
        vec![ServerMessage::Result(success_result(
            request,
            truth,
            ResultPayload::ConnectorTest(ConnectorTestResult {
                connector_id: connector,
                status: disposition,
                detail,
            }),
        ))],
        None,
    )
    .await;
    Ok(())
}

/// Runs the bounded read-only probe. Returns sanitized detail text either way;
/// credential values and remote bodies never appear in it.
fn require_probe_base_url<'a>(
    connector_id: &str,
    base_url: Option<&'a str>,
) -> Result<&'a str, String> {
    base_url.ok_or_else(|| {
        format!(
            "connector {connector_id} requires a configured base URL; set one with \
             connector.configure"
        )
    })
}

#[allow(clippy::too_many_lines)] // One flat branch per built-in connector keeps the probe auditable.
async fn run_connector_probe(
    connectors: &ConnectorRuntime,
    connector_id: &str,
    base_url: Option<&str>,
    token: Option<String>,
) -> Result<String, String> {
    let context = InvocationContext::new(
        Instant::now() + CONNECTOR_TEST_DEADLINE,
        CancellationToken::new(),
        1,
        None,
    )
    .map_err(|_| "connector probe context could not be constructed".to_owned())?;
    if connector_id == AWS {
        // The AWS probe needs no base URL and treats an absent stored value
        // as "use the CLI's default credential chain".
        let aws = connectors
            .aws(token.as_deref())
            .map_err(|error| error.message().to_owned())?;
        let probe = Connector::<AwsVerifyCredentials>::execute(
            &aws,
            AwsVerifyCredentialsRequest::default(),
            context,
        );
        await_connector_probe(probe).await?;
        return Ok("caller identity verified via the aws CLI".to_owned());
    }
    let Some(token) = token else {
        return Err(
            "no credential is stored for this connector; store one with connector.configure"
                .to_owned(),
        );
    };
    if connector_id == JENKINS {
        let base_url = require_probe_base_url(connector_id, base_url)?;
        let jenkins = connectors
            .jenkins(base_url, token)
            .map_err(|error| error.message().to_owned())?;
        let probe = Connector::<JenkinsVerifyCredentials>::execute(
            &jenkins,
            JenkinsVerifyCredentialsRequest::default(),
            context,
        );
        await_connector_probe(probe).await?;
        return Ok(format!("credential verified against {base_url}"));
    }
    if connector_id == SONARQUBE {
        let base_url = require_probe_base_url(connector_id, base_url)?;
        let sonarqube = connectors
            .sonarqube(base_url, token)
            .map_err(|error| error.message().to_owned())?;
        let probe = Connector::<SonarVerifyCredentials>::execute(
            &sonarqube,
            SonarVerifyCredentialsRequest::default(),
            context,
        );
        await_connector_probe(probe).await?;
        return Ok(format!("credential verified against {base_url}"));
    }
    if connector_id == JIRA {
        let base_url = require_probe_base_url(connector_id, base_url)?;
        let jira = connectors
            .jira(base_url, token)
            .map_err(|error| error.message().to_owned())?;
        let probe = Connector::<JiraVerifyCredentials>::execute(
            &jira,
            JiraVerifyCredentialsRequest::default(),
            context,
        );
        await_connector_probe(probe).await?;
        return Ok(format!("credential verified against {base_url}"));
    }
    if connector_id == CONFLUENCE {
        let base_url = require_probe_base_url(connector_id, base_url)?;
        let confluence = connectors
            .confluence(base_url, token)
            .map_err(|error| error.message().to_owned())?;
        let probe = Connector::<ConfluenceVerifyCredentials>::execute(
            &confluence,
            ConfluenceVerifyCredentialsRequest::default(),
            context,
        );
        await_connector_probe(probe).await?;
        return Ok(format!("credential verified against {base_url}"));
    }
    if connector_id == SHAREPOINT {
        let base_url = require_probe_base_url(connector_id, base_url)?;
        let sharepoint = connectors
            .sharepoint(base_url, token)
            .map_err(|error| error.message().to_owned())?;
        let probe = Connector::<SharePointVerifyCredentials>::execute(
            &sharepoint,
            SharePointVerifyCredentialsRequest::default(),
            context,
        );
        await_connector_probe(probe).await?;
        return Ok(format!("credential verified against {base_url}"));
    }
    let github = connectors
        .github(base_url, token)
        .map_err(|error| error.message().to_owned())?;
    let probe = Connector::<VerifyCredentials>::execute(
        &github,
        VerifyCredentialsRequest::default(),
        context,
    );
    await_connector_probe(probe).await?;
    Ok(format!(
        "credential verified against {}",
        base_url.unwrap_or(GITHUB_DEFAULT_API_BASE)
    ))
}

async fn await_connector_probe<T>(
    probe: impl Future<Output = Result<T, ConnectorFailure>>,
) -> Result<(), String> {
    let outcome = tokio::time::timeout(CONNECTOR_TEST_DEADLINE + Duration::from_secs(2), probe)
        .await
        .map_err(|_| "connector probe deadline elapsed".to_owned())?;
    outcome
        .map(drop)
        .map_err(|failure| connector_probe_failure(&failure))
}

fn connector_probe_failure(failure: &ConnectorFailure) -> String {
    format!("{:?}: {}", failure.kind(), failure.message())
}

fn bounded_connector_detail(mut detail: String) -> String {
    let mut length = detail.len().min(MAX_CONNECTOR_TEST_DETAIL_BYTES);
    while !detail.is_char_boundary(length) {
        length -= 1;
    }
    detail.truncate(length);
    detail
}

/// Maps the daemon's model surface into the caller-facing status contract.
///
/// `registered` carries the full durable registry catalog, so a model that is
/// imported but not loaded stays reachable. A loaded model the catalog does
/// not list is still reported: the status never hides what is actually
/// serving.
pub(super) fn model_status_result(
    loaded: Option<(&ModelKey, u64)>,
    registered: Vec<ModelSummary>,
    load_failure: Option<String>,
) -> Result<ModelStatusResult, pam_protocol::ProtocolContractError> {
    let loaded = loaded
        .map(|(key, size_bytes)| ModelSummary::new(key.id(), size_bytes))
        .transpose()?;
    let mut registered = registered;
    if let Some(loaded) = &loaded
        && !registered
            .iter()
            .any(|model| model.model_id() == loaded.model_id())
    {
        registered.push(loaded.clone());
    }
    Ok(ModelStatusResult {
        registered,
        loaded,
        load_failure,
    })
}

pub(super) fn protocol_caller_summary(registration: CallerRegistration) -> CallerSummary {
    CallerSummary {
        caller_id: registration.caller_id,
        registered_at_ms: registration.registered_at_ms,
        revoked_at_ms: registration.revoked_at_ms,
        kind: registration.kind,
    }
}

async fn handle_approval_decision(
    request: &RequestEnvelope,
    approval_id: ApprovalId,
    decision: ProtocolApprovalDecision,
    incoming: IncomingRequest,
    store: &Store,
    outbound: &mpsc::Sender<Outbound>,
) {
    let decision = match decision {
        ProtocolApprovalDecision::Approve => StoreApprovalDecision::Approve,
        ProtocolApprovalDecision::Deny => StoreApprovalDecision::Deny,
    };
    let outcome = store
        .decide_project_approval(
            approval_id.clone(),
            request.project_id.clone(),
            request.caller_id.clone(),
            decision,
            now_ms(),
        )
        .await;
    let disposition = match outcome {
        Ok(ApprovalDecisionOutcome::Approved) => ApprovalDecisionDisposition::Approved,
        Ok(ApprovalDecisionOutcome::Denied) => ApprovalDecisionDisposition::Denied,
        Ok(ApprovalDecisionOutcome::Expired) => ApprovalDecisionDisposition::Expired,
        Err(StoreError::ApprovalNotFound(_) | StoreError::InvalidApprovalState) => {
            send_routed(
                outbound,
                incoming,
                vec![ServerMessage::Result(failure_result(
                    request,
                    FailureCode::Forbidden,
                    "approval is unavailable for this project or caller",
                ))],
                None,
            )
            .await;
            return;
        }
        Err(error) => {
            send_store_failure(outbound, incoming, request, &error).await;
            return;
        }
    };
    send_routed(
        outbound,
        incoming,
        vec![ServerMessage::Result(success_result(
            request,
            OperationTruth::Changed,
            ResultPayload::ApprovalDecision(ApprovalDecisionResult {
                approval_id,
                disposition,
            }),
        ))],
        None,
    )
    .await;
}

async fn handle_status(
    request: &RequestEnvelope,
    incoming: IncomingRequest,
    store: &Store,
    outbound: &mpsc::Sender<Outbound>,
) -> Result<(), DaemonError> {
    let workload = match store.project_workload(request.project_id.clone()).await {
        Ok(workload) => workload,
        Err(error) => {
            send_store_failure(outbound, incoming, request, &error).await;
            return Ok(());
        }
    };
    send_routed(
        outbound,
        incoming,
        vec![ServerMessage::Result(ResultEnvelope {
            protocol_version: PROTOCOL_VERSION,
            request_id: request.request_id.clone(),
            project_id: request.project_id.clone(),
            body: ResultBody::Success {
                truth: OperationTruth::Observed,
                payload: ResultPayload::Status(StatusResult {
                    ready: true,
                    healthy: true,
                    daemon_version: APPLICATION_VERSION.to_owned(),
                    protocol_version: PROTOCOL_VERSION,
                    queue_depth: workload.queued,
                }),
            },
        })],
        None,
    )
    .await;
    Ok(())
}

async fn handle_durable_status(
    request: RequestEnvelope,
    incoming: IncomingRequest,
    store: &Store,
    outbound: &mpsc::Sender<Outbound>,
    scheduler: &mpsc::Sender<()>,
) -> Result<(), DaemonError> {
    let accepted = store
        .accept(
            AcceptRequest {
                request_id: request.request_id.clone(),
                caller_id: request.caller_id.clone(),
                project_id: request.project_id.clone(),
                idempotency_key: request.idempotency_key.clone(),
                operation_kind: "status".to_owned(),
                operation: Vec::new(),
            },
            now_ms(),
        )
        .await;
    let canonical_request_id = match accepted {
        Ok(
            AcceptOutcome::Created { request_id, .. } | AcceptOutcome::Existing { request_id, .. },
        ) => request_id,
        Err(error) => {
            send_store_failure(outbound, incoming, &request, &error).await;
            return Ok(());
        }
    };
    let replay = store.replay(canonical_request_id.clone(), 0).await?;
    let snapshot = store.snapshot(canonical_request_id.clone()).await?;
    let terminal = replay.result.is_some();
    let last_sequence = replay.events.last().map_or(0, |event| event.sequence);
    let messages = remap_messages(
        replay_messages(&snapshot.project_id, &canonical_request_id, replay)?,
        &request.request_id,
        &request.request_id,
        &request.project_id,
    );
    let subscription = (!terminal).then_some(SubscriptionRequest {
        canonical_request_id: canonical_request_id.clone(),
        event_request_id: request.request_id.clone(),
        observer_request_id: request.request_id.clone(),
        project_id: request.project_id.clone(),
        last_sequence,
    });
    if terminal {
        send_routed(outbound, incoming, messages, None).await;
    } else {
        let (registered_tx, registered_rx) = oneshot::channel();
        let _ = outbound
            .send(Outbound::Routed {
                incoming,
                messages,
                subscribe: subscription,
                registered: Some(registered_tx),
            })
            .await;
        if registered_rx.await.is_ok() {
            let replay = store.replay(canonical_request_id.clone(), 0).await?;
            let terminal = replay.result.is_some();
            let messages = replay_messages(&snapshot.project_id, &canonical_request_id, replay)?;
            let _ = outbound
                .send(Outbound::Persisted {
                    request_id: canonical_request_id,
                    messages,
                    terminal,
                })
                .await;
        }
        let _ = scheduler.send(()).await;
    }
    Ok(())
}

async fn handle_stop(
    request: &RequestEnvelope,
    incoming: IncomingRequest,
    outbound: &mpsc::Sender<Outbound>,
) {
    let _ = outbound
        .send(Outbound::Stop {
            incoming,
            result: Box::new(ResultEnvelope {
                protocol_version: PROTOCOL_VERSION,
                request_id: request.request_id.clone(),
                project_id: request.project_id.clone(),
                body: ResultBody::Success {
                    truth: OperationTruth::Changed,
                    payload: ResultPayload::DaemonLifecycle(DaemonLifecycleResult {
                        stopping: true,
                    }),
                },
            }),
        })
        .await;
}

async fn handle_flow_run(
    request: RequestEnvelope,
    prepared: PreparedFlowSubmission,
    resource: ResourceName,
    incoming: IncomingRequest,
    store: &Store,
    outbound: &mpsc::Sender<Outbound>,
    scheduler: &mpsc::Sender<()>,
) -> Result<(), DaemonError> {
    let RequestPayload::FlowRun { .. } = &request.payload else {
        unreachable!("flow execution is dispatched only for its matching payload")
    };
    let authorized = authorize_flow_submission(&request, prepared, &resource, store).await;
    let canonical_request_id = match authorized {
        Ok(FlowAuthorizationOutcome::Accepted(
            AcceptOutcome::Created { request_id, .. } | AcceptOutcome::Existing { request_id, .. },
        )) => request_id,
        Ok(outcome) => {
            send_routed(
                outbound,
                incoming,
                vec![ServerMessage::Result(flow_authorization_failure(
                    &request, &resource, outcome,
                ))],
                None,
            )
            .await;
            return Ok(());
        }
        Err(error) => {
            send_store_failure(outbound, incoming, &request, &error).await;
            return Ok(());
        }
    };
    let replay = store.replay(canonical_request_id.clone(), 0).await?;
    let snapshot = store.snapshot(canonical_request_id.clone()).await?;
    let terminal = replay.result.is_some();
    let last_sequence = replay.events.last().map_or(0, |event| event.sequence);
    let messages = remap_messages(
        replay_messages(&snapshot.project_id, &canonical_request_id, replay)?,
        &request.request_id,
        &request.request_id,
        &request.project_id,
    );
    if terminal {
        send_routed(outbound, incoming, messages, None).await;
        return Ok(());
    }

    let (registered_tx, registered_rx) = oneshot::channel();
    let _ = outbound
        .send(Outbound::Routed {
            incoming,
            messages,
            subscribe: Some(SubscriptionRequest {
                canonical_request_id: canonical_request_id.clone(),
                event_request_id: request.request_id.clone(),
                observer_request_id: request.request_id.clone(),
                project_id: request.project_id.clone(),
                last_sequence,
            }),
            registered: Some(registered_tx),
        })
        .await;
    if registered_rx.await.is_ok() {
        let replay = store
            .replay(canonical_request_id.clone(), last_sequence)
            .await?;
        let terminal = replay.result.is_some();
        let messages = replay_messages(&snapshot.project_id, &canonical_request_id, replay)?;
        let _ = outbound
            .send(Outbound::Persisted {
                request_id: canonical_request_id.clone(),
                messages,
                terminal,
            })
            .await;
    }
    let _ = scheduler.send(()).await;
    Ok(())
}

async fn authorize_flow_submission(
    request: &RequestEnvelope,
    prepared: PreparedFlowSubmission,
    resource: &ResourceName,
    store: &Store,
) -> Result<FlowAuthorizationOutcome, StoreError> {
    let now = now_ms();
    let audit_detail = redact_audit_detail(
        format!(
            "capability=flow.run resource={} detail=project policy evaluated",
            resource.as_str()
        )
        .as_bytes(),
    );
    store
        .authorize_flow_run(
            AuthorizeFlowRun {
                accept: AcceptRequest {
                    request_id: request.request_id.clone(),
                    caller_id: request.caller_id.clone(),
                    project_id: request.project_id.clone(),
                    idempotency_key: request.idempotency_key.clone(),
                    operation_kind: FLOW_OPERATION_KIND.to_owned(),
                    operation: prepared.operation,
                },
                resource: resource.clone(),
                approval_id: request.approval_id.clone(),
                audit: AuthorizationAudit {
                    event_id: request_audit_event_id(request, "policy", now),
                    action: "request.preflight".to_owned(),
                    redacted_detail: audit_detail,
                    retain_until_ms: now
                        .saturating_add(duration_ms(AUDIT_RETENTION))
                        .min(i64::MAX as u64),
                },
                schema_approval_required: prepared.schema_approval_required,
            },
            now,
            duration_ms(APPROVAL_LIFETIME),
        )
        .await
}

async fn handle_cancel(
    request: &RequestEnvelope,
    target_request_id: RequestId,
    expected_target_kind: Option<ExpectedTargetKind>,
    incoming: IncomingRequest,
    store: &Store,
    outbound: &mpsc::Sender<Outbound>,
    scheduler: &mpsc::Sender<()>,
) -> Result<(), DaemonError> {
    let snapshot =
        match target_snapshot(store, target_request_id.clone(), expected_target_kind).await {
            Ok(snapshot) if snapshot.project_id == request.project_id => snapshot,
            Ok(_) => {
                send_routed(
                    outbound,
                    incoming,
                    vec![ServerMessage::Result(failure_result(
                        request,
                        FailureCode::NotFound,
                        "target request was not found in this project",
                    ))],
                    None,
                )
                .await;
                return Ok(());
            }
            Err(StoreError::RequestNotFound(_)) => {
                send_routed(
                    outbound,
                    incoming,
                    vec![ServerMessage::Result(failure_result(
                        request,
                        FailureCode::NotFound,
                        "target request was not found",
                    ))],
                    None,
                )
                .await;
                return Ok(());
            }
            Err(error) => return Err(error.into()),
        };
    let target_project_id = snapshot.project_id;
    let stored = encoded_cancelled_result(&target_request_id, &target_project_id)?;
    let outcome = target_cancel(
        store,
        target_request_id.clone(),
        now_ms(),
        stored,
        expected_target_kind,
    )
    .await?;
    let (disposition, truth) = cancellation_presentation(outcome);
    let result = ResultEnvelope {
        protocol_version: PROTOCOL_VERSION,
        request_id: request.request_id.clone(),
        project_id: request.project_id.clone(),
        body: ResultBody::Success {
            truth,
            payload: ResultPayload::Cancellation(CancellationResult {
                target_request_id: target_request_id.clone(),
                disposition,
            }),
        },
    };
    send_routed(
        outbound,
        incoming,
        vec![ServerMessage::Result(result)],
        None,
    )
    .await;
    let replay = target_replay(store, target_request_id.clone(), 0, expected_target_kind).await?;
    let terminal = replay.result.is_some();
    let messages = replay_messages(&target_project_id, &target_request_id, replay)?;
    let _ = outbound
        .send(Outbound::Persisted {
            request_id: target_request_id,
            messages,
            terminal,
        })
        .await;
    let _ = scheduler.send(()).await;
    Ok(())
}

pub(super) fn cancellation_presentation(
    outcome: CancelOutcome,
) -> (CancellationDisposition, OperationTruth) {
    let disposition = match outcome {
        CancelOutcome::Cancelled | CancelOutcome::CancellationRequested => {
            CancellationDisposition::Requested
        }
        CancelOutcome::AlreadyRequested => CancellationDisposition::AlreadyRequested,
        CancelOutcome::AlreadyTerminal(RequestState::Cancelled) => {
            CancellationDisposition::AlreadyCancelled
        }
        CancelOutcome::AlreadyTerminal(_) => CancellationDisposition::AlreadyTerminal,
    };
    let truth = if disposition == CancellationDisposition::Requested {
        OperationTruth::Changed
    } else {
        OperationTruth::Observed
    };
    (disposition, truth)
}

fn encoded_cancelled_result(
    target_request_id: &RequestId,
    target_project_id: &ProjectId,
) -> Result<Vec<u8>, CodecError> {
    encode(&ServerMessage::Result(ResultEnvelope {
        protocol_version: PROTOCOL_VERSION,
        request_id: target_request_id.clone(),
        project_id: target_project_id.clone(),
        body: ResultBody::Failure(Failure {
            code: FailureCode::Cancelled,
            message: "request was cancelled".to_owned(),
            recovery: None,
            approval: None,
        }),
    }))
}

async fn target_snapshot(
    store: &Store,
    request_id: RequestId,
    expected_target_kind: Option<ExpectedTargetKind>,
) -> Result<RequestSnapshot, StoreError> {
    match expected_target_kind {
        None => store.snapshot(request_id).await,
        Some(ExpectedTargetKind::FlowRun) => {
            store
                .snapshot_with_expected_target(request_id, ExpectedOperationKind::FlowRun)
                .await
        }
    }
}

async fn target_replay(
    store: &Store,
    request_id: RequestId,
    after_sequence: u64,
    expected_target_kind: Option<ExpectedTargetKind>,
) -> Result<Replay, StoreError> {
    match expected_target_kind {
        None => store.replay(request_id, after_sequence).await,
        Some(ExpectedTargetKind::FlowRun) => {
            store
                .replay_with_expected_target(
                    request_id,
                    after_sequence,
                    ExpectedOperationKind::FlowRun,
                )
                .await
        }
    }
}

async fn target_cancel(
    store: &Store,
    request_id: RequestId,
    now_ms: u64,
    result: Vec<u8>,
    expected_target_kind: Option<ExpectedTargetKind>,
) -> Result<CancelOutcome, StoreError> {
    match expected_target_kind {
        None => store.cancel(request_id, now_ms, result).await,
        Some(ExpectedTargetKind::FlowRun) => {
            store
                .cancel_with_expected_target(
                    request_id,
                    now_ms,
                    result,
                    ExpectedOperationKind::FlowRun,
                )
                .await
        }
    }
}

async fn handle_replay(
    request: &RequestEnvelope,
    target_request_id: RequestId,
    after_sequence: u64,
    expected_target_kind: Option<ExpectedTargetKind>,
    incoming: IncomingRequest,
    store: &Store,
    outbound: &mpsc::Sender<Outbound>,
) -> Result<(), DaemonError> {
    if after_sequence > i64::MAX as u64 {
        send_routed(
            outbound,
            incoming,
            vec![ServerMessage::Result(failure_result(
                request,
                FailureCode::InvalidRequest,
                "replay sequence exceeds the supported range",
            ))],
            None,
        )
        .await;
        return Ok(());
    }
    let snapshot =
        match target_snapshot(store, target_request_id.clone(), expected_target_kind).await {
            Ok(snapshot) if snapshot.project_id == request.project_id => snapshot,
            Ok(_) | Err(StoreError::RequestNotFound(_)) => {
                send_routed(
                    outbound,
                    incoming,
                    vec![ServerMessage::Result(failure_result(
                        request,
                        FailureCode::NotFound,
                        "target request was not found in this project",
                    ))],
                    None,
                )
                .await;
                return Ok(());
            }
            Err(error) => return Err(error.into()),
        };
    let replay = target_replay(
        store,
        target_request_id.clone(),
        after_sequence,
        expected_target_kind,
    )
    .await?;
    let terminal = replay.result.is_some();
    let through_sequence = replay
        .events
        .last()
        .map_or(after_sequence, |event| event.sequence);
    let include_target_result = request.request_id == target_request_id;
    let mut messages =
        replay_messages_without_result(&snapshot.project_id, &target_request_id, &replay.events)?;
    if terminal && include_target_result {
        if let Some(result) = replay.result {
            messages.push(decode_stored_result(&result.payload)?);
        }
    } else {
        messages.push(ServerMessage::Result(ResultEnvelope {
            protocol_version: PROTOCOL_VERSION,
            request_id: request.request_id.clone(),
            project_id: request.project_id.clone(),
            body: ResultBody::Success {
                truth: OperationTruth::Observed,
                payload: ResultPayload::Replay(ReplayResult {
                    target_request_id,
                    through_sequence,
                    pending: !terminal,
                }),
            },
        }));
    }
    send_routed(outbound, incoming, messages, None).await;
    Ok(())
}

async fn handle_brief(
    request: &RequestEnvelope,
    incoming: IncomingRequest,
    store: &Store,
    outbound: &mpsc::Sender<Outbound>,
    provider: &dyn BriefProvider,
) -> Result<(), DaemonError> {
    let brief = provider.brief(&request.project_id, store).await;
    if !brief_is_bounded(&brief) {
        send_routed(
            outbound,
            incoming,
            vec![ServerMessage::Result(failure_result(
                request,
                FailureCode::FrameTooLarge,
                "brief provider response exceeded bounded limits",
            ))],
            None,
        )
        .await;
        return Ok(());
    }
    send_routed(
        outbound,
        incoming,
        vec![ServerMessage::Result(success_result(
            request,
            brief_truth(&brief),
            ResultPayload::Brief(brief),
        ))],
        None,
    )
    .await;
    Ok(())
}

async fn handle_wait_for_result(
    request: &RequestEnvelope,
    target_request_id: RequestId,
    after_sequence: u64,
    expected_target_kind: Option<ExpectedTargetKind>,
    incoming: IncomingRequest,
    store: &Store,
    outbound: &mpsc::Sender<Outbound>,
) -> Result<(), DaemonError> {
    if !valid_replay_cursor(after_sequence) {
        send_routed(
            outbound,
            incoming,
            vec![ServerMessage::Result(failure_result(
                request,
                FailureCode::InvalidRequest,
                "wait sequence exceeds the supported range",
            ))],
            None,
        )
        .await;
        return Ok(());
    }
    let snapshot =
        match target_snapshot(store, target_request_id.clone(), expected_target_kind).await {
            Ok(snapshot) if snapshot.project_id == request.project_id => snapshot,
            Ok(_) | Err(StoreError::RequestNotFound(_)) => {
                send_target_not_found(outbound, incoming, request).await;
                return Ok(());
            }
            Err(error) => return Err(error.into()),
        };
    let replay = target_replay(
        store,
        target_request_id.clone(),
        after_sequence,
        expected_target_kind,
    )
    .await?;
    let terminal = replay.result.is_some();
    let last_sequence = replay
        .events
        .last()
        .map_or(after_sequence, |event| event.sequence);
    let messages = wait_messages(request, &snapshot.project_id, &target_request_id, replay)?;
    if terminal {
        send_routed(outbound, incoming, messages, None).await;
        return Ok(());
    }

    let (registered_tx, registered_rx) = oneshot::channel();
    let _ = outbound
        .send(Outbound::Routed {
            incoming,
            messages,
            subscribe: Some(SubscriptionRequest {
                canonical_request_id: target_request_id.clone(),
                event_request_id: target_request_id.clone(),
                observer_request_id: request.request_id.clone(),
                project_id: request.project_id.clone(),
                last_sequence,
            }),
            registered: Some(registered_tx),
        })
        .await;
    if registered_rx.await.is_ok() {
        let replay = target_replay(
            store,
            target_request_id.clone(),
            last_sequence,
            expected_target_kind,
        )
        .await?;
        let terminal = replay.result.is_some();
        let messages = replay_messages(&snapshot.project_id, &target_request_id, replay)?;
        let _ = outbound
            .send(Outbound::Persisted {
                request_id: target_request_id,
                messages,
                terminal,
            })
            .await;
    }
    Ok(())
}

async fn handle_get_result(
    request: &RequestEnvelope,
    target_request_id: RequestId,
    expected_target_kind: Option<ExpectedTargetKind>,
    incoming: IncomingRequest,
    store: &Store,
    outbound: &mpsc::Sender<Outbound>,
) -> Result<(), DaemonError> {
    let snapshot =
        match target_snapshot(store, target_request_id.clone(), expected_target_kind).await {
            Ok(snapshot) if snapshot.project_id == request.project_id => snapshot,
            Ok(_) | Err(StoreError::RequestNotFound(_)) => {
                send_target_not_found(outbound, incoming, request).await;
                return Ok(());
            }
            Err(error) => return Err(error.into()),
        };
    let replay = target_replay(
        store,
        target_request_id.clone(),
        i64::MAX as u64,
        expected_target_kind,
    )
    .await?;
    let result = match replay.result {
        Some(stored) => remap_stored_result(
            &stored.payload,
            &target_request_id,
            &snapshot.project_id,
            &request.request_id,
        )?,
        None => failure_result(
            request,
            FailureCode::Pending,
            "target request has not completed",
        ),
    };
    send_routed(
        outbound,
        incoming,
        vec![ServerMessage::Result(result)],
        None,
    )
    .await;
    Ok(())
}

async fn handle_inspect_evidence(
    request: &RequestEnvelope,
    handle: EvidenceHandle,
    incoming: IncomingRequest,
    store: &Store,
    outbound: &mpsc::Sender<Outbound>,
) -> Result<(), DaemonError> {
    match store
        .inspect_evidence(request.project_id.clone(), handle)
        .await
    {
        Ok(metadata) => {
            send_routed(
                outbound,
                incoming,
                vec![ServerMessage::Result(success_result(
                    request,
                    OperationTruth::Observed,
                    ResultPayload::EvidenceMetadata(protocol_evidence_metadata(metadata)),
                ))],
                None,
            )
            .await;
        }
        Err(error) => send_evidence_failure(outbound, incoming, request, &error).await,
    }
    Ok(())
}

async fn handle_read_evidence(
    request: &RequestEnvelope,
    handle: EvidenceHandle,
    offset: u64,
    length: u64,
    incoming: IncomingRequest,
    store: &Store,
    outbound: &mpsc::Sender<Outbound>,
) -> Result<(), DaemonError> {
    let metadata = match store
        .inspect_evidence(request.project_id.clone(), handle.clone())
        .await
    {
        Ok(metadata) => metadata,
        Err(error) => {
            send_evidence_failure(outbound, incoming, request, &error).await;
            return Ok(());
        }
    };
    let bytes = match store
        .read_evidence_range(request.project_id.clone(), handle.clone(), offset, length)
        .await
    {
        Ok(bytes) => bytes,
        Err(error) => {
            send_evidence_failure(outbound, incoming, request, &error).await;
            return Ok(());
        }
    };
    let end = offset
        .checked_add(usize_to_u64(bytes.len()))
        .ok_or_else(|| StoreError::InvalidState("evidence range overflowed".to_owned()))?;
    let chunk = EvidenceChunk::new(handle, offset, bytes, end == metadata.size_bytes)
        .map_err(|error| StoreError::InvalidState(error.to_string()))?;
    send_routed(
        outbound,
        incoming,
        vec![ServerMessage::Result(success_result(
            request,
            OperationTruth::Observed,
            ResultPayload::EvidenceChunk(chunk),
        ))],
        None,
    )
    .await;
    Ok(())
}

fn wait_messages(
    request: &RequestEnvelope,
    project_id: &ProjectId,
    target_request_id: &RequestId,
    replay: Replay,
) -> Result<Vec<ServerMessage>, DaemonError> {
    let mut messages =
        replay_messages_without_result(project_id, target_request_id, &replay.events)?;
    if let Some(stored) = replay.result {
        messages.push(ServerMessage::Result(remap_stored_result(
            &stored.payload,
            target_request_id,
            project_id,
            &request.request_id,
        )?));
    }
    Ok(messages)
}

fn remap_stored_result(
    payload: &[u8],
    target_request_id: &RequestId,
    project_id: &ProjectId,
    observer_request_id: &RequestId,
) -> Result<ResultEnvelope, DaemonError> {
    let ServerMessage::Result(result) = decode_stored_result(payload)? else {
        unreachable!("decode_stored_result accepts only result messages")
    };
    if result.request_id != *target_request_id || result.project_id != *project_id {
        return Err(StoreError::InvalidState(
            "stored result correlation does not match its request".to_owned(),
        )
        .into());
    }
    Ok(ResultEnvelope {
        protocol_version: result.protocol_version,
        request_id: observer_request_id.clone(),
        project_id: project_id.clone(),
        body: result.body,
    })
}

fn success_result(
    request: &RequestEnvelope,
    truth: OperationTruth,
    payload: ResultPayload,
) -> ResultEnvelope {
    ResultEnvelope {
        protocol_version: PROTOCOL_VERSION,
        request_id: request.request_id.clone(),
        project_id: request.project_id.clone(),
        body: ResultBody::Success { truth, payload },
    }
}

async fn send_target_not_found(
    outbound: &mpsc::Sender<Outbound>,
    incoming: IncomingRequest,
    request: &RequestEnvelope,
) {
    send_routed(
        outbound,
        incoming,
        vec![ServerMessage::Result(failure_result(
            request,
            FailureCode::NotFound,
            "target request was not found in this project",
        ))],
        None,
    )
    .await;
}

async fn send_evidence_failure(
    outbound: &mpsc::Sender<Outbound>,
    incoming: IncomingRequest,
    request: &RequestEnvelope,
    error: &StoreError,
) {
    let (code, message) = match error {
        StoreError::EvidenceNotFound { .. } => (
            FailureCode::NotFound,
            "evidence was not found in this project",
        ),
        StoreError::EvidenceRangeTooLarge { .. } | StoreError::EvidenceRangeOutOfBounds { .. } => {
            (FailureCode::InvalidRequest, "evidence range is invalid")
        }
        StoreError::EvidenceBlobMissing(_)
        | StoreError::EvidenceBlobCorrupt(_)
        | StoreError::UnsafeEvidencePath => (
            FailureCode::Internal,
            "evidence is unavailable or failed integrity verification",
        ),
        _ => (FailureCode::Internal, "evidence storage is unavailable"),
    };
    send_routed(
        outbound,
        incoming,
        vec![ServerMessage::Result(failure_result(
            request, code, message,
        ))],
        None,
    )
    .await;
}

fn protocol_evidence_metadata(metadata: pam_store::EvidenceMetadata) -> EvidenceMetadata {
    EvidenceMetadata {
        handle: metadata.handle,
        digest: metadata.digest,
        size_bytes: metadata.size_bytes,
        media_type: metadata.media_type,
        retention: match metadata.retention {
            pam_store::EvidenceRetention::Session => EvidenceRetention::Session,
            pam_store::EvidenceRetention::Project => EvidenceRetention::Project,
            pam_store::EvidenceRetention::Persistent => EvidenceRetention::Persistent,
        },
        redaction: match metadata.redaction {
            pam_store::EvidenceRedaction::Unredacted => EvidenceRedaction::Unredacted,
            pam_store::EvidenceRedaction::Redacted => EvidenceRedaction::Redacted,
        },
        created_at_unix_ms: metadata.created_at_ms,
    }
}

const fn valid_replay_cursor(sequence: u64) -> bool {
    sequence <= i64::MAX as u64
}

fn request_identifiers_are_bounded(request: &RequestEnvelope) -> bool {
    [
        request.request_id.as_str(),
        request.caller_id.as_str(),
        request.project_id.as_str(),
        request.idempotency_key.as_str(),
    ]
    .into_iter()
    .all(identifier_is_bounded)
        && request
            .approval_id
            .as_ref()
            .is_none_or(|approval_id| identifier_is_bounded(approval_id.as_str()))
        && target_request_id(request).is_none_or(|target| identifier_is_bounded(target.as_str()))
        && match &request.payload {
            RequestPayload::ApprovalDecide { approval_id, .. } => {
                identifier_is_bounded(approval_id.as_str())
            }
            _ => true,
        }
}

fn target_request_id(request: &RequestEnvelope) -> Option<&RequestId> {
    match &request.payload {
        RequestPayload::Cancel {
            target_request_id, ..
        }
        | RequestPayload::Replay {
            target_request_id, ..
        }
        | RequestPayload::WaitForResult {
            target_request_id, ..
        }
        | RequestPayload::GetResult {
            target_request_id, ..
        } => Some(target_request_id),
        RequestPayload::Status
        | RequestPayload::Stop
        | RequestPayload::DaemonActivity { .. }
        | RequestPayload::DaemonLogs { .. }
        | RequestPayload::DaemonStats { .. }
        | RequestPayload::CallerList
        | RequestPayload::ProjectCurrent
        | RequestPayload::ApprovalDecide { .. }
        | RequestPayload::Brief
        | RequestPayload::NetworkDiagnostics
        | RequestPayload::InspectEvidence { .. }
        | RequestPayload::ReadEvidence { .. }
        | RequestPayload::ModelInfer { .. }
        | RequestPayload::ModelStatus
        | RequestPayload::ModelRegister { .. }
        | RequestPayload::ModelUnregister { .. }
        | RequestPayload::ModelVerify { .. }
        | RequestPayload::ModelSweep
        | RequestPayload::ModelDeleteWeights { .. }
        | RequestPayload::GrantRevoke { .. }
        | RequestPayload::FlowRun { .. }
        | RequestPayload::ConnectorList
        | RequestPayload::ConnectorConfigure { .. }
        | RequestPayload::ConnectorTest { .. }
        | RequestPayload::ResetAccess { .. }
        | RequestPayload::ResetIdentity { .. }
        | RequestPayload::ResetHistory { .. }
        | RequestPayload::ResetRegistry { .. } => None,
    }
}

fn identifier_is_bounded(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_REQUEST_IDENTIFIER_BYTES
        && !value.chars().any(is_unsafe_identifier_character)
}

fn is_unsafe_identifier_character(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            '\u{00ad}'
                | '\u{0600}'..='\u{0605}'
                | '\u{061c}'
                | '\u{06dd}'
                | '\u{070f}'
                | '\u{0890}'..='\u{0891}'
                | '\u{08e2}'
                | '\u{180e}'
                | '\u{200b}'..='\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2060}'..='\u{2064}'
                | '\u{2066}'..='\u{206f}'
                | '\u{feff}'
                | '\u{fff9}'..='\u{fffb}'
                | '\u{110bd}'
                | '\u{110cd}'
                | '\u{13430}'..='\u{1343f}'
                | '\u{1bca0}'..='\u{1bca3}'
                | '\u{1d173}'..='\u{1d17a}'
                | '\u{e0001}'
                | '\u{e0020}'..='\u{e007f}'
        )
}

fn brief_is_bounded(brief: &BriefResult) -> bool {
    brief.goal.as_ref().is_none_or(brief_item_is_bounded)
        && brief.decisions.len() <= MAX_BRIEF_SECTION_ITEMS
        && brief.decisions.iter().all(brief_item_is_bounded)
        && brief.verified.len() <= MAX_BRIEF_SECTION_ITEMS
        && brief.verified.iter().all(brief_item_is_bounded)
        && brief.next.len() <= MAX_BRIEF_SECTION_ITEMS
        && brief.next.iter().all(brief_item_is_bounded)
        && brief.provenance.len() <= MAX_BRIEF_PROVENANCE_ITEMS
        && brief.provenance.iter().all(|entry| {
            entry.source.len() <= MAX_BRIEF_SOURCE_BYTES
                && entry
                    .detail
                    .as_ref()
                    .is_none_or(|detail| detail.len() <= MAX_BRIEF_DETAIL_BYTES)
        })
}

fn brief_truth(brief: &BriefResult) -> OperationTruth {
    if brief.provenance.is_empty() {
        return OperationTruth::Unresolved;
    }
    if brief
        .provenance
        .iter()
        .any(|source| source.truth == OperationTruth::Blocked)
    {
        OperationTruth::Blocked
    } else if brief
        .provenance
        .iter()
        .any(|source| source.truth == OperationTruth::Unresolved)
    {
        OperationTruth::Unresolved
    } else {
        OperationTruth::Observed
    }
}

fn brief_item_is_bounded(item: &pam_protocol::BriefItem) -> bool {
    item.text.len() <= MAX_BRIEF_TEXT_BYTES && item.evidence.len() <= MAX_BRIEF_EVIDENCE_HANDLES
}

/// Opportunistically remembers the caller's project root so caller histories
/// can later label this project by location instead of its opaque ID.
///
/// Best-effort and silent: an absent, invalid, or mismatched root never fails
/// the request it rode in on. Validated exactly like a flow run's project
/// root (canonical, absolute, and discovering back to this exact project ID)
/// before ever reaching durable state.
async fn learn_project_root(request: &RequestEnvelope, store: &Store) {
    if request.project_id.is_daemon_scope() {
        return;
    }
    let Some(project_root) = &request.project_root else {
        return;
    };
    let Ok(canonical_root) =
        verify_flow_project_root(Path::new(project_root.as_str()), &request.project_id)
    else {
        return;
    };
    let Some(root) = canonical_root.to_str() else {
        return;
    };
    let _ = store
        .remember_project_root(request.project_id.clone(), root.to_owned())
        .await;
}

/// One durable-store error as the failure the caller sees.
fn store_failure_result(request: &RequestEnvelope, error: &StoreError) -> ResultEnvelope {
    let (code, message) = match error {
        StoreError::IdempotencyConflict { .. } => {
            (FailureCode::IdempotencyConflict, error.to_string())
        }
        StoreError::RequestIdConflict(_) => (FailureCode::InvalidRequest, error.to_string()),
        _ => (FailureCode::Internal, error.to_string()),
    };
    failure_result(request, code, &message)
}

async fn send_store_failure(
    outbound: &mpsc::Sender<Outbound>,
    incoming: IncomingRequest,
    request: &RequestEnvelope,
    error: &StoreError,
) {
    send_routed(
        outbound,
        incoming,
        vec![ServerMessage::Result(store_failure_result(request, error))],
        None,
    )
    .await;
}

async fn send_routed(
    outbound: &mpsc::Sender<Outbound>,
    incoming: IncomingRequest,
    messages: Vec<ServerMessage>,
    subscribe: Option<SubscriptionRequest>,
) {
    let _ = outbound
        .send(Outbound::Routed {
            incoming,
            messages,
            subscribe,
            registered: None,
        })
        .await;
}

async fn run_scheduler(
    store: Store,
    mut wakeups: mpsc::Receiver<()>,
    outbound: mpsc::Sender<Outbound>,
    processing_delay: Duration,
    connectors: ConnectorRuntime,
    log: DaemonLog,
) -> Result<(), DaemonError> {
    let mut workers = JoinSet::new();
    let mut recovery = tokio::time::interval(RECOVERY_INTERVAL);
    recovery.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let owner = format!("daemon-{}", std::process::id());
    loop {
        // The scheduler must outlive individual failures: a bad pass or a
        // failed run is logged and retried on the next tick, never fatal.
        if let Err(error) = scheduler_pass(
            &store,
            &outbound,
            &owner,
            processing_delay,
            &connectors,
            &mut workers,
        )
        .await
        {
            log.error(format!("scheduler pass failed: {error}"));
        }

        if wakeups.is_closed() && workers.is_empty() {
            return Ok(());
        }

        tokio::select! {
            _ = recovery.tick() => {}
            wakeup = wakeups.recv() => {
                if wakeup.is_none() && workers.is_empty() {
                    return Ok(());
                }
            }
            completed = workers.join_next(), if !workers.is_empty() => {
                match completed {
                    Some(Ok(Ok(()))) | None => {}
                    Some(Ok(Err(error))) => {
                        log.error(format!("queued operation failed: {error}"));
                    }
                    Some(Err(error)) => {
                        if !error.is_cancelled() {
                            log.error(format!("queued operation panicked: {error}"));
                        }
                    }
                }
            }
        }
    }
}

/// One recovery-and-claim sweep. Errors abort only this pass.
async fn scheduler_pass(
    store: &Store,
    outbound: &mpsc::Sender<Outbound>,
    owner: &str,
    processing_delay: Duration,
    connectors: &ConnectorRuntime,
    workers: &mut JoinSet<Result<(), DaemonError>>,
) -> Result<(), DaemonError> {
    for request_id in store.recover_expired_requests(now_ms()).await? {
        let snapshot = store.snapshot(request_id.clone()).await?;
        let replay = store.replay(request_id.clone(), 0).await?;
        let terminal = replay.result.is_some();
        let messages = replay_messages(&snapshot.project_id, &request_id, replay)?;
        let _ = outbound
            .send(Outbound::Persisted {
                request_id,
                messages,
                terminal,
            })
            .await;
    }
    loop {
        let leased = match store
            .claim(owner, now_ms(), duration_ms(LEASE_DURATION))
            .await
        {
            Ok(Some(leased)) => leased,
            Ok(None) => break Ok(()),
            Err(StoreError::CorruptFlowAuthorization(request_id)) => {
                quarantine_corrupt_flow_authorization(store, outbound, request_id).await?;
                continue;
            }
            Err(error) => break Err(error.into()),
        };
        let worker_store = store.clone();
        let worker_outbound = outbound.clone();
        let worker_connectors = connectors.clone();
        workers.spawn(async move {
            process_leased(
                leased,
                worker_store,
                worker_outbound,
                processing_delay,
                LEASE_DURATION,
                worker_connectors,
            )
            .await
        });
    }
}

async fn quarantine_corrupt_flow_authorization(
    store: &Store,
    outbound: &mpsc::Sender<Outbound>,
    request_id: RequestId,
) -> Result<(), DaemonError> {
    let snapshot = store.snapshot(request_id.clone()).await?;
    let result = ServerMessage::Result(ResultEnvelope {
        protocol_version: PROTOCOL_VERSION,
        request_id: request_id.clone(),
        project_id: snapshot.project_id.clone(),
        body: ResultBody::Failure(Failure {
            code: FailureCode::Internal,
            message: "stored flow authorization is invalid".to_owned(),
            recovery: None,
            approval: None,
        }),
    });
    let recovery = store
        .fail_corrupt_flow_authorization(request_id.clone(), now_ms(), encode(&result)?)
        .await?;
    if matches!(recovery, FlowAuthorizationRecoveryOutcome::NoLongerEligible) {
        return Ok(());
    }
    let replay = store.replay(request_id.clone(), 0).await?;
    let messages = replay_messages(&snapshot.project_id, &request_id, replay)?;
    let _ = outbound
        .send(Outbound::Persisted {
            request_id,
            messages,
            terminal: true,
        })
        .await;
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn process_leased(
    mut leased: LeasedRequest,
    store: Store,
    outbound: mpsc::Sender<Outbound>,
    processing_delay: Duration,
    lease_duration: Duration,
    connectors: ConnectorRuntime,
) -> Result<(), DaemonError> {
    if !processing_delay.is_zero() {
        let mut processing = std::pin::pin!(tokio::time::sleep(processing_delay));
        let mut heartbeat = tokio::time::interval(LEASE_HEARTBEAT);
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // The first interval tick is immediate; the initial lease is already live.
        heartbeat.tick().await;
        loop {
            tokio::select! {
                () = &mut processing => break,
                _ = heartbeat.tick() => {
                    match store
                        .renew(
                            leased.lease.clone(),
                            now_ms(),
                            duration_ms(lease_duration),
                        )
                        .await
                    {
                        Ok(lease) => {
                            leased.lease = lease;
                            if store
                                .snapshot(leased.lease.request_id.clone())
                                .await?
                                .state
                                == RequestState::CancellationRequested
                            {
                                break;
                            }
                        }
                        Err(StoreError::StaleLease(_)) => return Ok(()),
                        Err(error) => return Err(error.into()),
                    }
                }
            }
        }
    }
    let queue_depth = store.queued_behind(leased.lease.request_id.clone()).await?;
    let (terminal_state, result, cached_flow_terminal, encoded_flow_result) =
        if matches!(leased.operation_kind.as_str(), "daemon_status" | "status") {
            (
                TerminalState::Succeeded,
                ResultEnvelope {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: leased.lease.request_id.clone(),
                    project_id: leased.lease.project_id.clone(),
                    body: ResultBody::Success {
                        truth: OperationTruth::Observed,
                        payload: ResultPayload::Status(StatusResult {
                            ready: true,
                            healthy: true,
                            daemon_version: APPLICATION_VERSION.to_owned(),
                            protocol_version: PROTOCOL_VERSION,
                            queue_depth,
                        }),
                    },
                },
                false,
                None,
            )
        } else if leased.operation_kind == FLOW_OPERATION_KIND {
            match process_flow(
                &mut leased,
                &store,
                lease_duration,
                LEASE_HEARTBEAT,
                &connectors,
            )
            .await
            {
                Err(StoreError::StaleLease(_)) | Ok(FlowProcessing::StaleLease) => return Ok(()),
                Err(error) => return Err(error.into()),
                Ok(FlowProcessing::Terminal {
                    terminal_state,
                    result,
                    encoded_result,
                    ..
                }) => (
                    terminal_state,
                    *result,
                    !encoded_result.is_empty(),
                    (!encoded_result.is_empty()).then_some(encoded_result),
                ),
            }
        } else {
            (
                TerminalState::Failed,
                ResultEnvelope {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: leased.lease.request_id.clone(),
                    project_id: leased.lease.project_id.clone(),
                    body: ResultBody::Failure(Failure {
                        code: FailureCode::InvalidRequest,
                        message: format!("unknown durable operation {}", leased.operation_kind),
                        recovery: None,
                        approval: None,
                    }),
                },
                false,
                None,
            )
        };
    let stored = if let Some(encoded) = encoded_flow_result {
        encoded
    } else {
        encode(&ServerMessage::Result(result))?
    };
    let finished = if cached_flow_terminal {
        store
            .finish_terminal_flow(leased.lease.clone(), now_ms(), stored)
            .await
    } else {
        store
            .finish(leased.lease.clone(), now_ms(), terminal_state, stored)
            .await
    };
    match finished {
        Ok(_) => {}
        Err(StoreError::StaleLease(_)) => return Ok(()),
        Err(error) => return Err(error.into()),
    }
    let replay = store.replay(leased.lease.request_id.clone(), 0).await?;
    let messages = replay_messages(&leased.lease.project_id, &leased.lease.request_id, replay)?;
    let _ = outbound
        .send(Outbound::Persisted {
            request_id: leased.lease.request_id,
            messages,
            terminal: true,
        })
        .await;
    Ok(())
}

fn replay_messages(
    project_id: &ProjectId,
    request_id: &RequestId,
    replay: Replay,
) -> Result<Vec<ServerMessage>, DaemonError> {
    let mut messages = replay_messages_without_result(project_id, request_id, &replay.events)?;
    if let Some(result) = replay.result {
        let message = decode_stored_result(&result.payload)?;
        let ServerMessage::Result(envelope) = &message else {
            unreachable!("decode_stored_result accepts only result messages")
        };
        if envelope.request_id != *request_id || envelope.project_id != *project_id {
            return Err(StoreError::InvalidState(
                "stored result correlation does not match its request".to_owned(),
            )
            .into());
        }
        messages.push(message);
    }
    Ok(messages)
}

fn replay_messages_without_result(
    project_id: &ProjectId,
    request_id: &RequestId,
    events: &[EventRecord],
) -> Result<Vec<ServerMessage>, DaemonError> {
    events
        .iter()
        .map(|record| {
            Ok(ServerMessage::Event(EventEnvelope {
                protocol_version: PROTOCOL_VERSION,
                request_id: request_id.clone(),
                project_id: project_id.clone(),
                sequence: record.sequence,
                event: stored_event(record)?,
            }))
        })
        .collect()
}

pub(super) fn decode_stored_result(payload: &[u8]) -> Result<ServerMessage, DaemonError> {
    let mut message = decode_server_message_envelope(payload)?;
    if let ServerMessage::Result(result) = &mut message {
        if result.protocol_version == 0 || result.protocol_version > PROTOCOL_VERSION {
            return Err(StoreError::InvalidState(
                "stored result uses an unsupported protocol version".to_owned(),
            )
            .into());
        }
        if matches!(
            &result.body,
            ResultBody::Success {
                payload: ResultPayload::FlowRun(_),
                ..
            }
        ) && result.protocol_version < 4
        {
            return Err(StoreError::InvalidState(
                "stored flow result predates the flow protocol".to_owned(),
            )
            .into());
        }
        // Durable results outlive the transport version that first encoded
        // them. Their typed payload is decoded compatibly, then the daemon
        // re-envelopes the replay for the current observer protocol.
        if result.protocol_version < PROTOCOL_VERSION
            && let ResultBody::Success {
                truth,
                payload: ResultPayload::FlowRun(flow),
            } = &mut result.body
        {
            *truth = flow_result_truth(flow);
        }
        result.protocol_version = PROTOCOL_VERSION;
        return Ok(message);
    }
    Err(StoreError::InvalidState("stored result is not a result envelope".to_owned()).into())
}

fn stored_event(record: &EventRecord) -> Result<Event, DaemonError> {
    if record.kind.starts_with("flow_") {
        return Ok(Event::FlowTransition(decode_flow_transition(
            &record.payload,
        )?));
    }
    match record.kind.as_str() {
        "accepted" => Ok(Event::Accepted),
        "started" => Ok(Event::Started),
        "lease_expired" => Ok(Event::LeaseExpired),
        "cancellation_requested" => Ok(Event::CancellationRequested),
        "cancelled" => Ok(Event::Cancelled),
        "completed" => Ok(Event::Completed),
        "failed" => Ok(Event::Failed),
        other => Err(StoreError::InvalidState(format!("unknown event kind {other}")).into()),
    }
}

fn failure_result(request: &RequestEnvelope, code: FailureCode, message: &str) -> ResultEnvelope {
    ResultEnvelope {
        protocol_version: PROTOCOL_VERSION,
        request_id: request.request_id.clone(),
        project_id: request.project_id.clone(),
        body: ResultBody::Failure(Failure {
            code,
            message: message.to_owned(),
            recovery: None,
            approval: None,
        }),
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn duration_ms(duration: Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}

fn usize_to_u64(value: usize) -> u64 {
    u64::try_from(value).expect("usize fits into u64 on supported targets")
}

pub(super) fn prepare_endpoint(config: &DaemonConfig) -> Result<(), DaemonError> {
    if let Some(socket_path) = config.endpoint.socket_path()
        && socket_path.exists()
    {
        if config.recover {
            remove_if_present(socket_path)?;
        } else {
            return Err(DaemonError::StaleState(
                "Unix socket path already exists".to_owned(),
            ));
        }
    }
    Ok(())
}

fn remove_if_present(path: &Path) -> Result<(), DaemonError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

enum ServeAction {
    Shutdown,
    Incoming(Result<IncomingRequest, TransportError>),
    Outbound(Option<Outbound>),
    HandlerCompleted(Option<Result<Result<(), DaemonError>, tokio::task::JoinError>>),
    SchedulerCompleted(Result<Result<(), DaemonError>, tokio::task::JoinError>),
}

pub(super) struct Ownership {
    _file: fs::File,
}

impl Ownership {
    pub(super) fn acquire(endpoint: &LocalEndpoint) -> Result<Self, DaemonError> {
        let runtime = open_private_runtime_dir(endpoint.runtime_dir())?;
        let ownership_name = endpoint
            .ownership_path()
            .strip_prefix(endpoint.runtime_dir())
            .ok()
            .filter(|path| path.components().count() == 1)
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "daemon ownership path must be a direct runtime-directory child",
                )
            })?;
        let mut options = OpenOptions::new();
        options
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .follow(FollowSymlinks::No);
        let mut file = runtime.open_with(ownership_name, &options)?.into_std();
        file.try_lock().map_err(|error| match error {
            fs::TryLockError::WouldBlock => DaemonError::AlreadyRunning,
            fs::TryLockError::Error(error) => DaemonError::Io(error),
        })?;
        file.set_len(0)?;
        writeln!(file, "{}", std::process::id())?;
        Ok(Self { _file: file })
    }
}

fn open_private_runtime_dir(path: &Path) -> Result<Dir, std::io::Error> {
    if !path.is_absolute() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "daemon runtime directory must be absolute",
        ));
    }
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "daemon runtime directory must have a parent",
        )
    })?;
    let name = path.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "daemon runtime directory must have a final component",
        )
    })?;

    fs::create_dir_all(parent)?;
    #[cfg(unix)]
    let builder = {
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        builder
    };
    #[cfg(not(unix))]
    let builder = fs::DirBuilder::new();
    match builder.create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }

    let parent = Dir::open_ambient_dir(parent, ambient_authority())?;
    let runtime = parent.open_dir_nofollow(name)?;
    #[cfg(unix)]
    harden_unix_runtime_dir(&runtime)?;
    Ok(runtime)
}

#[cfg(unix)]
fn harden_unix_runtime_dir(runtime: &Dir) -> Result<(), std::io::Error> {
    let metadata = runtime.dir_metadata()?;
    if metadata.uid() != Uid::effective().as_raw() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "daemon runtime directory is owned by another user",
        ));
    }

    runtime.set_permissions(".", cap_std::fs::Permissions::from_mode(0o700))?;
    if runtime.dir_metadata()?.permissions().mode() & 0o077 != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "daemon runtime directory is accessible by another user",
        ));
    }
    Ok(())
}
