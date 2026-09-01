use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use pam_client::request_exchange;
use pam_core::{
    CallerCredential, CallerId, ContentDigest, GrantId, IdempotencyKey, ProjectId, RequestId,
};
use pam_daemon::{DaemonConfig, serve_until};
use pam_platform::LocalEndpoint;
use pam_policy::{ApprovalRequirement, CapabilityName, Effect, Grant, ResourceScope};
use pam_protocol::{
    Capability, FailureCode, ModelRegistration, OperationTruth, RequestEnvelope, ResultBody,
    ResultPayload,
};
use pam_store::Store;
use tokio::{sync::oneshot, task::JoinHandle};

const TEST_CREDENTIAL: &str = "daemon-scope-caller-credential";
/// How long a test waits for one exchange to come back.
///
/// This is the client's patience, not an assertion: every exchange in this file
/// asserts the *result* it gets, never that the exchange timed out. A healthy
/// daemon answers in milliseconds, so the number costs nothing until something
/// is genuinely slow. Daemon startup is not charged here either —
/// `wait_until_ready` pays for it before the first asserted exchange.
///
/// It has to clear two budgets that live underneath it, and fifteen seconds sat
/// on top of both:
///
/// * The client opens a fresh connection per exchange, and `zeromq`'s
///   `connect_forever` retries a refused or not-yet-listening endpoint on an
///   exponential back-off — roughly 1.4s, 2.0s, 2.7s, 3.8s, 5.3s — summing to
///   **15.15s** before the sixth attempt. A 15s deadline sits 0.15s *below*
///   that, so an exchange needing the full cycle is guaranteed to lose the race
///   and to report a flat ~15.2s `DeadlineExceeded` on a trivial request.
/// * `model.load` initializes the llama.cpp backend, which on macOS compiles
///   ggml's embedded Metal library. Warm, that costs ~0.01s; cold, with the
///   machine oversubscribed, it measured **14.6s to 15.8s** across 24
///   concurrent copies of this binary — and all of it is charged to whichever
///   model exchange runs first. A CI runner is always cold.
///
/// So this must not be "tidied" back to 15s: fifteen is the one value certain
/// to fire in the middle of work that was about to succeed.
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(45);

fn test_runtime(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let base = if cfg!(unix) {
        PathBuf::from("/tmp")
    } else {
        std::env::temp_dir()
    };
    base.join(format!(
        "pam-it-{name}-{}-{}",
        std::process::id(),
        nonce % 1_000_000
    ))
}

fn start_daemon(
    endpoint: LocalEndpoint,
) -> (
    oneshot::Sender<()>,
    JoinHandle<Result<(), pam_daemon::DaemonError>>,
) {
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let state_path = endpoint.runtime_dir().join("state.sqlite3");
    let daemon = tokio::spawn(serve_until(
        DaemonConfig {
            endpoint,
            recover: false,
            model: None,
            model_from_default: false,
            state_path: Some(state_path),
            brief_provider: None,
            connector_secret_backend: None,
        },
        async {
            let _ = shutdown_rx.await;
        },
    ));
    (shutdown_tx, daemon)
}

/// How long a test waits for a freshly started daemon to answer.
///
/// The endpoint is bound last: the store opens and migrates, the credential
/// store warms and any configured model loads first. A shared CI runner
/// stretches every one of those, so readiness is given more patience than any
/// single exchange.
const READY_TIMEOUT: Duration = Duration::from_mins(1);
/// How long one readiness probe waits before it is retried.
const READY_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
/// How long readiness pauses between probes.
const READY_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Waits until the daemon answers, not merely until its socket file appears.
///
/// A socket file proves the listener is bound, never that the daemon serves,
/// and the cost of that gap lands on whichever exchange goes first. The
/// transport's connect back-off is exponential — roughly 1.4s, then 2.0s, 2.7s,
/// 3.8s and 5.3s — so an endpoint that is late by a fraction of a second costs
/// seconds and one that is late by ten spends the whole 15.15s retry budget.
/// Waiting for a complete round trip here keeps startup out of the exchange
/// deadlines, which then cover only the exchange they are spent on.
///
/// The probe is deliberately inert. Its capability and payload do not match, so
/// the daemon answers on shape alone — before authentication, policy or the
/// audit ledger — and leaves nothing behind for any test to observe.
///
/// Readiness that never arrives panics here, naming the endpoint and the wait,
/// instead of falling through into a confusing deadline further down.
async fn wait_until_ready(endpoint: &LocalEndpoint) {
    let started = Instant::now();
    let mut probes = 0_u32;
    loop {
        if endpoint.socket_path().is_some_and(std::path::Path::exists) {
            probes += 1;
            if let Ok(exchange) =
                request_exchange(endpoint, &readiness_probe(probes), READY_PROBE_TIMEOUT).await
            {
                assert!(
                    matches!(
                        &exchange.result.body,
                        ResultBody::Failure(failure) if failure.code == FailureCode::InvalidRequest
                    ),
                    "the readiness probe must stay inert: {:?}",
                    exchange.result.body
                );
                return;
            }
        }
        assert!(
            started.elapsed() < READY_TIMEOUT,
            "daemon at {} answered no readiness probe within {READY_TIMEOUT:?} ({probes} tried)",
            endpoint.address()
        );
        tokio::time::sleep(READY_POLL_INTERVAL).await;
    }
}

/// A request the daemon must answer and must not record.
fn readiness_probe(probe: u32) -> RequestEnvelope {
    let mut request = RequestEnvelope::status(
        RequestId::new(format!("readiness-probe-{probe}")),
        CallerId::from("readiness-probe"),
        ProjectId::from("readiness-probe"),
        IdempotencyKey::new(format!("readiness-probe-{probe}")),
    );
    // `Status` pairs only with `DaemonStatus`; under any other capability the
    // daemon rejects the request for its shape without touching the store.
    request.capability = Capability::CallerList;
    request
}

async fn seed_caller(store: &Store, caller: &str, credential: &str) {
    store
        .register_caller(CallerId::from(caller), CallerCredential::new(credential), 1)
        .await
        .unwrap();
}

fn authenticated(request: RequestEnvelope) -> RequestEnvelope {
    request.authenticated(CallerCredential::new(TEST_CREDENTIAL))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::too_many_lines)] // One fixture proves the complete daemon-scope boundary.
async fn daemon_scope_serves_daemon_reads_without_any_project() {
    let runtime = test_runtime("daemon-scope-round-trip");
    let endpoint = LocalEndpoint::ipc(runtime.clone());
    let state_path = endpoint.runtime_dir().join("state.sqlite3");

    // No project is registered anywhere: the store only knows the caller.
    let seed = Store::open(&state_path).unwrap();
    seed_caller(&seed, "scope-operator", TEST_CREDENTIAL).await;
    seed.shutdown().await.unwrap();

    let (shutdown, daemon) = start_daemon(endpoint.clone());
    wait_until_ready(&endpoint).await;

    let scope = ProjectId::daemon_scope();
    let caller = CallerId::from("scope-operator");

    // Every daemon-scoped baseline read succeeds under the reserved scope.
    let activity = request_exchange(
        &endpoint,
        &authenticated(RequestEnvelope::daemon_activity(
            RequestId::from("scope-activity"),
            caller.clone(),
            scope.clone(),
            IdempotencyKey::from("scope-activity-key"),
            0,
        )),
        EXCHANGE_TIMEOUT,
    )
    .await
    .unwrap();
    assert!(matches!(
        activity.result.body,
        ResultBody::Success {
            truth: OperationTruth::Observed,
            payload: ResultPayload::DaemonActivity(_),
        }
    ));

    let logs = request_exchange(
        &endpoint,
        &authenticated(RequestEnvelope::daemon_logs(
            RequestId::from("scope-logs"),
            caller.clone(),
            scope.clone(),
            IdempotencyKey::from("scope-logs-key"),
            0,
        )),
        EXCHANGE_TIMEOUT,
    )
    .await
    .unwrap();
    match logs.result.body {
        ResultBody::Success {
            truth: OperationTruth::Observed,
            payload: ResultPayload::DaemonLogs(logs),
        } => {
            // The startup "ready" line is always present in the ring.
            assert!(
                logs.entries
                    .iter()
                    .any(|entry| entry.message.contains("Pam daemon ready"))
            );
        }
        other => panic!("daemon logs read failed: {other:?}"),
    }

    let stats = request_exchange(
        &endpoint,
        &authenticated(RequestEnvelope::daemon_stats(
            RequestId::from("scope-stats"),
            caller.clone(),
            scope.clone(),
            IdempotencyKey::from("scope-stats-key"),
            0,
        )),
        EXCHANGE_TIMEOUT,
    )
    .await
    .unwrap();
    match &stats.result.body {
        ResultBody::Success {
            truth: OperationTruth::Observed,
            payload: ResultPayload::DaemonStats(_),
        } => {}
        other => panic!("daemon stats read failed: {other:?}"),
    }

    let callers = request_exchange(
        &endpoint,
        &authenticated(RequestEnvelope::caller_list(
            RequestId::from("scope-callers"),
            caller.clone(),
            scope.clone(),
            IdempotencyKey::from("scope-callers-key"),
        )),
        EXCHANGE_TIMEOUT,
    )
    .await
    .unwrap();
    assert!(matches!(
        callers.result.body,
        ResultBody::Success {
            payload: ResultPayload::CallerList(_),
            ..
        }
    ));

    let model = request_exchange(
        &endpoint,
        &authenticated(RequestEnvelope::model_status(
            RequestId::from("scope-model"),
            caller.clone(),
            scope.clone(),
            IdempotencyKey::from("scope-model-key"),
        )),
        EXCHANGE_TIMEOUT,
    )
    .await
    .unwrap();
    assert!(matches!(
        model.result.body,
        ResultBody::Success {
            payload: ResultPayload::ModelStatus(_),
            ..
        }
    ));

    let connectors = request_exchange(
        &endpoint,
        &authenticated(RequestEnvelope::connector_list(
            RequestId::from("scope-connectors"),
            caller.clone(),
            scope.clone(),
            IdempotencyKey::from("scope-connectors-key"),
        )),
        EXCHANGE_TIMEOUT,
    )
    .await
    .unwrap();
    assert!(matches!(
        connectors.result.body,
        ResultBody::Success {
            payload: ResultPayload::ConnectorList(_),
            ..
        }
    ));

    // The audit ledger recorded those requests under the "daemon" project and
    // the activity feed replays them as such.
    let replayed = request_exchange(
        &endpoint,
        &authenticated(RequestEnvelope::daemon_activity(
            RequestId::from("scope-activity-after"),
            caller.clone(),
            scope.clone(),
            IdempotencyKey::from("scope-activity-after-key"),
            0,
        )),
        EXCHANGE_TIMEOUT,
    )
    .await
    .unwrap();
    let ResultBody::Success {
        payload: ResultPayload::DaemonActivity(feed),
        ..
    } = replayed.result.body
    else {
        panic!("a daemon-scope activity read should return a typed result")
    };
    assert!(
        feed.events
            .iter()
            .any(|event| event.project_id.as_str() == "daemon"
                && event.caller_id.as_str() == "scope-operator"),
        "daemon-scope requests must appear in the feed with project \"daemon\""
    );

    // A project-scoped capability rejects the reserved scope cleanly.
    let rejected = request_exchange(
        &endpoint,
        &authenticated(RequestEnvelope::project_current(
            RequestId::from("scope-project-current"),
            caller,
            scope,
            IdempotencyKey::from("scope-project-current-key"),
        )),
        EXCHANGE_TIMEOUT,
    )
    .await
    .unwrap();
    let ResultBody::Failure(failure) = rejected.result.body else {
        panic!("project.current must reject the reserved daemon scope")
    };
    assert_eq!(failure.code, FailureCode::InvalidRequest);
    assert!(
        failure.message.contains("needs a project"),
        "rejection must say a project is needed: {}",
        failure.message
    );

    shutdown.send(()).unwrap();
    daemon.await.unwrap().unwrap();
    let _ = fs::remove_dir_all(runtime);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn daemon_scope_grant_authorizes_connector_configure() {
    let runtime = test_runtime("daemon-scope-grant-round-trip");
    let endpoint = LocalEndpoint::ipc(runtime.clone());
    let state_path = endpoint.runtime_dir().join("state.sqlite3");

    let seed = Store::open(&state_path).unwrap();
    seed_caller(&seed, "scope-operator", TEST_CREDENTIAL).await;
    // The grant is recorded against the reserved daemon scope, not a project.
    seed.put_grant(pam_store::PutGrant {
        grant: Grant {
            id: GrantId::from("daemon-scope-configure-grant"),
            caller: CallerId::from("scope-operator"),
            project: ProjectId::daemon_scope(),
            capability: CapabilityName::parse("connector.configure").unwrap(),
            resource: ResourceScope::Any,
            effect: Effect::Allow,
            approval: ApprovalRequirement::None,
            expires_at_ms: None,
            revoked_at_ms: None,
        },
        created_at_ms: 2,
    })
    .await
    .unwrap();
    seed.shutdown().await.unwrap();

    let (shutdown, daemon) = start_daemon(endpoint.clone());
    wait_until_ready(&endpoint).await;

    let configure = |request_id: &str, project: ProjectId| {
        authenticated(
            RequestEnvelope::connector_configure(
                RequestId::from(request_id),
                CallerId::from("scope-operator"),
                project,
                IdempotencyKey::new(format!("{request_id}-key")),
                "github-actions",
                Some(true),
                None,
                None,
            )
            .unwrap(),
        )
    };

    // The daemon-scope grant authorizes the daemon-scope configure.
    let configured = request_exchange(
        &endpoint,
        &configure("scope-configure", ProjectId::daemon_scope()),
        EXCHANGE_TIMEOUT,
    )
    .await
    .unwrap();
    let ResultBody::Success {
        truth,
        payload: ResultPayload::ConnectorConfigure(configured),
    } = configured.result.body
    else {
        panic!("a daemon-scope grant should authorize a daemon-scope configure")
    };
    assert_eq!(truth, OperationTruth::Changed);
    assert!(configured.connector.enabled);

    // The same grant does not leak into a real project's scope.
    let denied = request_exchange(
        &endpoint,
        &configure("project-configure", ProjectId::from("project-elsewhere")),
        EXCHANGE_TIMEOUT,
    )
    .await
    .unwrap();
    assert!(matches!(
        denied.result.body,
        ResultBody::Failure(ref failure) if failure.code == FailureCode::Forbidden
    ));

    shutdown.send(()).unwrap();
    daemon.await.unwrap().unwrap();
    let _ = fs::remove_dir_all(runtime);
}

/// The GUI's Access view reads the observed boundary over the daemon
/// authority, with no project anywhere. This exercises that exact path:
/// scope admission first, then policy, then the typed read.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn daemon_scope_grant_authorizes_the_access_boundary_read() {
    let runtime = test_runtime("daemon-scope-network-round-trip");
    let endpoint = LocalEndpoint::ipc(runtime.clone());
    let state_path = endpoint.runtime_dir().join("state.sqlite3");

    // No project is registered: the store knows the caller and one
    // daemon-scope grant, nothing else.
    let seed = Store::open(&state_path).unwrap();
    seed_caller(&seed, "scope-operator", TEST_CREDENTIAL).await;
    seed.put_grant(pam_store::PutGrant {
        grant: Grant {
            id: GrantId::from("daemon-scope-network-grant"),
            caller: CallerId::from("scope-operator"),
            project: ProjectId::daemon_scope(),
            capability: CapabilityName::parse("network.diagnostics").unwrap(),
            resource: ResourceScope::Any,
            effect: Effect::Allow,
            approval: ApprovalRequirement::None,
            expires_at_ms: None,
            revoked_at_ms: None,
        },
        created_at_ms: 2,
    })
    .await
    .unwrap();
    seed.shutdown().await.unwrap();

    let (shutdown, daemon) = start_daemon(endpoint.clone());
    wait_until_ready(&endpoint).await;

    let observed = request_exchange(
        &endpoint,
        &authenticated(RequestEnvelope::network_diagnostics(
            RequestId::from("scope-network"),
            CallerId::from("scope-operator"),
            ProjectId::daemon_scope(),
            IdempotencyKey::from("scope-network-key"),
        )),
        // Reading the native trust store is a security-server round trip, not a
        // durable read like the other daemon-scope probes in this file.
        EXCHANGE_TIMEOUT,
    )
    .await
    .unwrap();
    match observed.result.body {
        ResultBody::Success {
            payload: ResultPayload::NetworkDiagnostics(_),
            ..
        } => {}
        other => panic!("a daemon-scope access-boundary read must be served: {other:?}"),
    }

    shutdown.send(()).unwrap();
    daemon.await.unwrap().unwrap();
    let _ = fs::remove_dir_all(runtime);
}

fn registration() -> ModelRegistration {
    ModelRegistration {
        model: "qwen/routed-model".to_owned(),
        path: "/models/qwen/routed-model.gguf".to_owned(),
        digest: ContentDigest::from_sha256([7; 32]).as_str().to_owned(),
        size_bytes: 64,
        gguf_version: 3,
        gguf_tensor_count: 17,
        gguf_metadata_kv_count: 29,
        license_id: "Apache-2.0".to_owned(),
        license_url: "https://example.test/license".to_owned(),
        license_digest: ContentDigest::from_sha256([8; 32]).as_str().to_owned(),
        source_url: None,
        registered_at_ms: 5,
    }
}

fn register_request(request_id: &str, caller: &str) -> RequestEnvelope {
    authenticated(
        RequestEnvelope::model_register(
            RequestId::from(request_id),
            CallerId::from(caller),
            ProjectId::daemon_scope(),
            IdempotencyKey::new(format!("{request_id}-key")),
            registration(),
        )
        .unwrap(),
    )
}

fn unregister_request(request_id: &str, caller: &str, model: &str) -> RequestEnvelope {
    authenticated(
        RequestEnvelope::model_unregister(
            RequestId::from(request_id),
            CallerId::from(caller),
            ProjectId::daemon_scope(),
            IdempotencyKey::new(format!("{request_id}-key")),
            model,
        )
        .unwrap(),
    )
}

async fn daemon_scope_grant(store: &Store, id: &str, caller: &str, capability: &str) {
    store
        .put_grant(pam_store::PutGrant {
            grant: Grant {
                id: GrantId::from(id),
                caller: CallerId::from(caller),
                project: ProjectId::daemon_scope(),
                capability: CapabilityName::parse(capability).unwrap(),
                resource: ResourceScope::Any,
                effect: Effect::Allow,
                approval: ApprovalRequirement::None,
                expires_at_ms: None,
                revoked_at_ms: None,
            },
            created_at_ms: 2,
        })
        .await
        .unwrap();
}

/// The GUI registers a model it has already verified through this exact
/// envelope, so the authorization boundary is proven where it actually runs:
/// a real request through the daemon's own dispatch, not a DTO in a unit test.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn model_register_needs_its_grant_and_then_writes_the_registry() {
    let runtime = test_runtime("daemon-scope-model-register");
    let endpoint = LocalEndpoint::ipc(runtime.clone());
    let state_path = endpoint.runtime_dir().join("state.sqlite3");

    let seed = Store::open(&state_path).unwrap();
    seed_caller(&seed, "scope-operator", TEST_CREDENTIAL).await;
    seed_caller(&seed, "ungranted-operator", TEST_CREDENTIAL).await;
    daemon_scope_grant(
        &seed,
        "daemon-scope-register-grant",
        "scope-operator",
        "model.register",
    )
    .await;
    seed.shutdown().await.unwrap();

    let (shutdown, daemon) = start_daemon(endpoint.clone());
    wait_until_ready(&endpoint).await;

    // `model.register` is not baseline: an ungranted caller is refused.
    let denied = request_exchange(
        &endpoint,
        &register_request("scope-register-denied", "ungranted-operator"),
        EXCHANGE_TIMEOUT,
    )
    .await
    .unwrap();
    assert!(
        matches!(denied.result.body, ResultBody::Failure(ref failure) if failure.code == FailureCode::Forbidden),
        "an ungranted caller must not register a model: {:?}",
        denied.result.body
    );

    // The granted caller registers, and registering again is idempotent.
    let registered = request_exchange(
        &endpoint,
        &register_request("scope-register", "scope-operator"),
        EXCHANGE_TIMEOUT,
    )
    .await
    .unwrap();
    let ResultBody::Success {
        truth,
        payload: ResultPayload::ModelRegister(acknowledged),
    } = registered.result.body
    else {
        panic!("a granted daemon-scope registration must be served");
    };
    assert_eq!(truth, OperationTruth::Changed);
    assert_eq!(acknowledged.model, "qwen/routed-model");
    assert_eq!(acknowledged.registered_at_ms, 5);

    let again = request_exchange(
        &endpoint,
        &register_request("scope-register-again", "scope-operator"),
        EXCHANGE_TIMEOUT,
    )
    .await
    .unwrap();
    assert!(matches!(
        again.result.body,
        ResultBody::Success {
            payload: ResultPayload::ModelRegister(_),
            ..
        }
    ));

    shutdown.send(()).unwrap();
    daemon.await.unwrap().unwrap();

    // The daemon, not the client, wrote the registry.
    let store = Store::open(&state_path).unwrap();
    let models = store.list_models().await.unwrap();
    store.shutdown().await.unwrap();
    assert_eq!(
        models
            .iter()
            .map(|model| model.key.id())
            .collect::<Vec<_>>(),
        vec!["qwen/routed-model".to_owned()]
    );

    let _ = fs::remove_dir_all(runtime);
}

fn load_request(request_id: &str, caller: &str, model: &str) -> RequestEnvelope {
    authenticated(
        RequestEnvelope::model_load(
            RequestId::from(request_id),
            CallerId::from(caller),
            ProjectId::daemon_scope(),
            IdempotencyKey::new(format!("{request_id}-key")),
            model,
        )
        .unwrap(),
    )
}

fn unload_request(request_id: &str, caller: &str) -> RequestEnvelope {
    authenticated(RequestEnvelope::model_unload(
        RequestId::from(request_id),
        CallerId::from(caller),
        ProjectId::daemon_scope(),
        IdempotencyKey::new(format!("{request_id}-key")),
    ))
}

fn status_request(request_id: &str, caller: &str) -> RequestEnvelope {
    authenticated(RequestEnvelope::model_status(
        RequestId::from(request_id),
        CallerId::from(caller),
        ProjectId::daemon_scope(),
        IdempotencyKey::new(format!("{request_id}-key")),
    ))
}

/// Loading and unloading are the two halves of changing a model without
/// restarting, and both are grant-gated. This drives the real envelopes
/// through the daemon's own dispatch: the refusals, the not-found answers, and
/// the load failure that leaves the daemon serving and saying why.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::too_many_lines)] // One daemon proves the whole load/unload boundary.
async fn model_load_and_unload_are_granted_separately_and_never_stop_the_daemon() {
    let runtime = test_runtime("daemon-scope-model-load");
    let endpoint = LocalEndpoint::ipc(runtime.clone());
    let state_path = endpoint.runtime_dir().join("state.sqlite3");

    let seed = Store::open(&state_path).unwrap();
    seed_caller(&seed, "scope-operator", TEST_CREDENTIAL).await;
    seed_caller(&seed, "ungranted-operator", TEST_CREDENTIAL).await;
    for (id, capability) in [
        ("daemon-scope-load-register-grant", "model.register"),
        ("daemon-scope-load-grant", "model.load"),
        ("daemon-scope-unload-grant", "model.unload"),
    ] {
        daemon_scope_grant(&seed, id, "scope-operator", capability).await;
    }
    seed.shutdown().await.unwrap();

    let (shutdown, daemon) = start_daemon(endpoint.clone());
    wait_until_ready(&endpoint).await;

    request_exchange(
        &endpoint,
        &register_request("scope-load-seed", "scope-operator"),
        EXCHANGE_TIMEOUT,
    )
    .await
    .unwrap();

    // Neither capability is baseline: an ungranted caller is refused, and the
    // refusal names the exact grant that would allow it.
    let denied = request_exchange(
        &endpoint,
        &load_request(
            "scope-load-denied",
            "ungranted-operator",
            "qwen/routed-model",
        ),
        EXCHANGE_TIMEOUT,
    )
    .await
    .unwrap();
    let ResultBody::Failure(failure) = denied.result.body else {
        panic!("an ungranted caller must not load a model");
    };
    assert_eq!(failure.code, FailureCode::Forbidden);
    assert_eq!(
        failure.recovery.as_deref(),
        Some("pam access grant model.load --daemon --resource model:qwen/routed-model")
    );

    let denied = request_exchange(
        &endpoint,
        &unload_request("scope-unload-denied", "ungranted-operator"),
        EXCHANGE_TIMEOUT,
    )
    .await
    .unwrap();
    let ResultBody::Failure(failure) = denied.result.body else {
        panic!("an ungranted caller must not unload a model");
    };
    assert_eq!(failure.code, FailureCode::Forbidden);
    assert_eq!(
        failure.recovery.as_deref(),
        Some("pam access grant model.unload --daemon --resource models:loaded")
    );

    // Nothing is loaded, so unloading has nothing to drop and says so.
    let empty = request_exchange(
        &endpoint,
        &unload_request("scope-unload-empty", "scope-operator"),
        EXCHANGE_TIMEOUT,
    )
    .await
    .unwrap();
    let ResultBody::Failure(failure) = empty.result.body else {
        panic!("unloading nothing must be a plain not-found");
    };
    assert_eq!(failure.code, FailureCode::NotFound);
    assert_eq!(failure.message, "no model is loaded in this daemon");
    assert_eq!(
        failure.recovery.as_deref(),
        Some("load one with `pam model load <vendor/name>` before unloading")
    );

    // A model that was never registered is a not-found, and the registry read
    // happens before anything is unloaded.
    let missing = request_exchange(
        &endpoint,
        &load_request("scope-load-missing", "scope-operator", "qwen/absent"),
        EXCHANGE_TIMEOUT,
    )
    .await
    .unwrap();
    assert!(
        matches!(missing.result.body, ResultBody::Failure(ref failure)
            if failure.code == FailureCode::NotFound),
        "an unregistered model must report not-found: {:?}",
        missing.result.body
    );

    // The registered row points at weights that are not there, so the load
    // fails exactly the way a startup load failure does: the request reports
    // it, and the daemon keeps serving.
    let drifted = request_exchange(
        &endpoint,
        &load_request("scope-load-drifted", "scope-operator", "qwen/routed-model"),
        EXCHANGE_TIMEOUT,
    )
    .await
    .unwrap();
    let ResultBody::Failure(failure) = drifted.result.body else {
        panic!("weights that are not there cannot load");
    };
    assert_eq!(failure.code, FailureCode::Internal);
    assert!(
        failure
            .message
            .starts_with("model load failed; the daemon will serve without a model:"),
        "a runtime load failure must read like a startup one: {}",
        failure.message
    );
    assert_eq!(
        failure.recovery.as_deref(),
        Some(
            "run `pam model verify qwen/routed-model` to see what changed under the registration, then load again"
        )
    );

    // The daemon is still answering, still reports the catalog, and keeps the
    // reason on its surface rather than only in a log line.
    let status = request_exchange(
        &endpoint,
        &status_request("scope-load-status", "scope-operator"),
        EXCHANGE_TIMEOUT,
    )
    .await
    .unwrap();
    let ResultBody::Success {
        truth,
        payload: ResultPayload::ModelStatus(surface),
    } = status.result.body
    else {
        panic!("a daemon that could not load a model still answers model.status");
    };
    assert_eq!(truth, OperationTruth::Observed);
    assert!(surface.loaded.is_none());
    assert!(surface.transition.is_none());
    assert!(
        surface
            .load_failure
            .as_deref()
            .is_some_and(|reason| reason.contains("the daemon will serve without a model")),
        "the failed load must stay reportable: {:?}",
        surface.load_failure
    );
    assert_eq!(
        surface
            .registered
            .iter()
            .map(pam_protocol::ModelSummary::model_id)
            .collect::<Vec<_>>(),
        vec!["qwen/routed-model"]
    );

    shutdown.send(()).unwrap();
    daemon.await.unwrap().unwrap();
    let _ = fs::remove_dir_all(runtime);
}

/// Unregistering is the disposal half of the registry, and it is refused
/// until the owner grants it: this drives the real envelope through the
/// daemon's own dispatch, then proves the row and the audit line.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
// One daemon proves the whole disposal boundary: refusal, not-found, removal.
#[allow(clippy::too_many_lines)]
async fn model_unregister_needs_its_grant_and_then_removes_the_registry_row() {
    let runtime = test_runtime("daemon-scope-model-unregister");
    let endpoint = LocalEndpoint::ipc(runtime.clone());
    let state_path = endpoint.runtime_dir().join("state.sqlite3");

    let seed = Store::open(&state_path).unwrap();
    seed_caller(&seed, "scope-operator", TEST_CREDENTIAL).await;
    seed_caller(&seed, "ungranted-operator", TEST_CREDENTIAL).await;
    daemon_scope_grant(
        &seed,
        "daemon-scope-register-grant",
        "scope-operator",
        "model.register",
    )
    .await;
    daemon_scope_grant(
        &seed,
        "daemon-scope-unregister-grant",
        "scope-operator",
        "model.unregister",
    )
    .await;
    seed.shutdown().await.unwrap();

    let (shutdown, daemon) = start_daemon(endpoint.clone());
    wait_until_ready(&endpoint).await;

    request_exchange(
        &endpoint,
        &register_request("scope-unregister-seed", "scope-operator"),
        EXCHANGE_TIMEOUT,
    )
    .await
    .unwrap();

    // `model.unregister` is not baseline: an ungranted caller is refused, and
    // the refusal carries the exact grant command.
    let denied = request_exchange(
        &endpoint,
        &unregister_request(
            "scope-unregister-denied",
            "ungranted-operator",
            "qwen/routed-model",
        ),
        EXCHANGE_TIMEOUT,
    )
    .await
    .unwrap();
    let ResultBody::Failure(failure) = denied.result.body else {
        panic!("an ungranted caller must not unregister a model");
    };
    assert_eq!(failure.code, FailureCode::Forbidden);
    assert_eq!(
        failure.recovery.as_deref(),
        Some("pam access grant model.unregister --daemon --resource model:qwen/routed-model")
    );

    // A model that was never registered is a plain not-found.
    let missing = request_exchange(
        &endpoint,
        &unregister_request("scope-unregister-missing", "scope-operator", "qwen/absent"),
        EXCHANGE_TIMEOUT,
    )
    .await
    .unwrap();
    assert!(
        matches!(missing.result.body, ResultBody::Failure(ref failure) if failure.code == FailureCode::NotFound),
        "an unregistered model must report not-found: {:?}",
        missing.result.body
    );

    // The granted caller removes the row, and the acknowledgement describes
    // exactly what left the registry.
    let removed = request_exchange(
        &endpoint,
        &unregister_request("scope-unregister", "scope-operator", "qwen/routed-model"),
        EXCHANGE_TIMEOUT,
    )
    .await
    .unwrap();
    let ResultBody::Success {
        truth,
        payload: ResultPayload::ModelUnregister(acknowledged),
    } = removed.result.body
    else {
        panic!("a granted daemon-scope unregistration must be served");
    };
    assert_eq!(truth, OperationTruth::Changed);
    assert_eq!(acknowledged.model, "qwen/routed-model");
    assert_eq!(acknowledged.size_bytes, 64);
    assert_eq!(
        acknowledged.digest,
        ContentDigest::from_sha256([7; 32]).as_str()
    );

    shutdown.send(()).unwrap();
    daemon.await.unwrap().unwrap();

    // The daemon, not the client, wrote the registry — and left an audit line
    // for the change it made.
    let store = Store::open(&state_path).unwrap();
    let models = store.list_models().await.unwrap();
    let events = store
        .export_audit_events(ProjectId::daemon_scope(), 0, None, 100)
        .await
        .unwrap();
    store.shutdown().await.unwrap();
    assert!(
        models.is_empty(),
        "the registry row must be gone: {models:?}"
    );
    let unregistered = events
        .events
        .iter()
        .find(|event| event.action == "model.unregister")
        .expect("unregistering must leave a changed-truth audit line");
    assert_eq!(unregistered.decision, "allow");
    assert_eq!(unregistered.outcome, "unregistered");
    assert!(
        unregistered
            .redacted_detail
            .contains("model=qwen/routed-model"),
        "the audit detail must name the model: {}",
        unregistered.redacted_detail
    );
    assert!(unregistered.redacted_detail.contains("size_bytes=64"));

    let _ = fs::remove_dir_all(runtime);
}

/// Revocation is baseline because it is bound to the requesting caller: this
/// sends the real envelope for one caller and proves the other caller's
/// identical grant survives it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grant_revoke_drops_only_the_requesting_callers_own_grants() {
    let runtime = test_runtime("daemon-scope-grant-revoke");
    let endpoint = LocalEndpoint::ipc(runtime.clone());
    let state_path = endpoint.runtime_dir().join("state.sqlite3");

    let seed = Store::open(&state_path).unwrap();
    seed_caller(&seed, "scope-operator", TEST_CREDENTIAL).await;
    seed_caller(&seed, "other-operator", TEST_CREDENTIAL).await;
    // Neither caller holds a `grant.revoke` grant: admission comes from the
    // policy baseline alone.
    daemon_scope_grant(&seed, "own-infer-grant", "scope-operator", "model.infer").await;
    daemon_scope_grant(&seed, "other-infer-grant", "other-operator", "model.infer").await;
    seed.shutdown().await.unwrap();

    let (shutdown, daemon) = start_daemon(endpoint.clone());
    wait_until_ready(&endpoint).await;

    let revoked = request_exchange(
        &endpoint,
        &authenticated(
            RequestEnvelope::grant_revoke(
                RequestId::from("scope-revoke"),
                CallerId::from("scope-operator"),
                ProjectId::daemon_scope(),
                IdempotencyKey::from("scope-revoke-key"),
                "model.infer",
            )
            .unwrap(),
        ),
        EXCHANGE_TIMEOUT,
    )
    .await
    .unwrap();
    let ResultBody::Success {
        truth,
        payload: ResultPayload::GrantRevoke(result),
    } = revoked.result.body
    else {
        panic!("a baseline revocation of the caller's own grant must be served");
    };
    assert_eq!(truth, OperationTruth::Changed);
    assert_eq!(result.capability, "model.infer");
    assert_eq!(result.revoked, 1);

    shutdown.send(()).unwrap();
    daemon.await.unwrap().unwrap();

    let store = Store::open(&state_path).unwrap();
    let now = 3;
    let own = store
        .active_grants(
            CallerId::from("scope-operator"),
            ProjectId::daemon_scope(),
            now,
        )
        .await
        .unwrap();
    let other = store
        .active_grants(
            CallerId::from("other-operator"),
            ProjectId::daemon_scope(),
            now,
        )
        .await
        .unwrap();
    store.shutdown().await.unwrap();
    assert!(own.is_empty(), "the requester's own grant must be gone");
    assert_eq!(
        other.len(),
        1,
        "another caller's identical grant must survive"
    );

    let _ = fs::remove_dir_all(runtime);
}

/// Registry health is three separate capabilities over one registry, and the
/// most important thing about them is what they refuse. This drives all three
/// real envelopes through the daemon's own dispatch against a registration
/// whose file was never there: verification reports the exact failure, the
/// sweep sees the dangling row, and deleting the weights of a GGUF Pam only
/// ever verified in place is refused in words that say why.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::too_many_lines)] // One fixture proves all three health capabilities.
async fn registry_health_verifies_sweeps_and_refuses_to_delete_a_file_pam_never_downloaded() {
    let runtime = test_runtime("daemon-scope-model-health");
    let endpoint = LocalEndpoint::ipc(runtime.clone());
    let state_path = endpoint.runtime_dir().join("state.sqlite3");

    let seed = Store::open(&state_path).unwrap();
    seed_caller(&seed, "scope-operator", TEST_CREDENTIAL).await;
    seed_caller(&seed, "ungranted-operator", TEST_CREDENTIAL).await;
    for (id, capability) in [
        ("daemon-scope-health-register", "model.register"),
        ("daemon-scope-health-verify", "model.verify"),
        ("daemon-scope-health-sweep", "model.sweep"),
        ("daemon-scope-health-delete", "model.delete-weights"),
    ] {
        daemon_scope_grant(&seed, id, "scope-operator", capability).await;
    }
    seed.shutdown().await.unwrap();

    let (shutdown, daemon) = start_daemon(endpoint.clone());
    wait_until_ready(&endpoint).await;

    request_exchange(
        &endpoint,
        &register_request("health-register", "scope-operator"),
        EXCHANGE_TIMEOUT,
    )
    .await
    .unwrap();

    // Verification is not baseline: an ungranted caller is refused outright.
    let denied = request_exchange(
        &endpoint,
        &authenticated(
            RequestEnvelope::model_verify(
                RequestId::from("health-verify-denied"),
                CallerId::from("ungranted-operator"),
                ProjectId::daemon_scope(),
                IdempotencyKey::from("health-verify-denied-key"),
                None,
            )
            .unwrap(),
        ),
        EXCHANGE_TIMEOUT,
    )
    .await
    .unwrap();
    assert!(
        matches!(denied.result.body, ResultBody::Failure(ref failure) if failure.code == FailureCode::Forbidden),
        "an ungranted caller must not verify the registry: {:?}",
        denied.result.body
    );

    // The registered path was never written, so the catalog verifies to an
    // unresolved truth naming the exact check that failed.
    let verified = request_exchange(
        &endpoint,
        &authenticated(
            RequestEnvelope::model_verify(
                RequestId::from("health-verify"),
                CallerId::from("scope-operator"),
                ProjectId::daemon_scope(),
                IdempotencyKey::from("health-verify-key"),
                None,
            )
            .unwrap(),
        ),
        EXCHANGE_TIMEOUT,
    )
    .await
    .unwrap();
    let ResultBody::Success {
        truth,
        payload: ResultPayload::ModelVerify(report),
    } = verified.result.body
    else {
        panic!("a granted verification must be served: {verified:?}");
    };
    assert_eq!(truth, OperationTruth::Unresolved);
    let entry = report
        .models
        .iter()
        .find(|model| model.model == "qwen/routed-model")
        .expect("the registered model must appear in the report");
    assert_eq!(entry.health, "path_missing");
    assert!(entry.detail.is_some(), "a failure must explain itself");
    assert_eq!(entry.source, "local");
    assert!(
        !entry.weights_deletable,
        "a GGUF Pam verified in place is never Pam's to delete"
    );

    // The sweep sees the same row from the other direction.
    let swept = request_exchange(
        &endpoint,
        &authenticated(RequestEnvelope::model_sweep(
            RequestId::from("health-sweep"),
            CallerId::from("scope-operator"),
            ProjectId::daemon_scope(),
            IdempotencyKey::from("health-sweep-key"),
        )),
        EXCHANGE_TIMEOUT,
    )
    .await
    .unwrap();
    let ResultBody::Success {
        truth,
        payload: ResultPayload::ModelSweep(sweep),
    } = swept.result.body
    else {
        panic!("a granted sweep must be served: {swept:?}");
    };
    assert_eq!(truth, OperationTruth::Observed);
    assert!(
        sweep
            .dangling
            .iter()
            .any(|row| row.model == "qwen/routed-model" && row.size_bytes == 64),
        "the sweep must report the row whose file is gone: {:?}",
        sweep.dangling
    );

    // Deleting the weights of a locally imported model is refused, and the
    // refusal says Pam did not download the file plus what to do instead.
    let refused = request_exchange(
        &endpoint,
        &authenticated(
            RequestEnvelope::model_delete_weights(
                RequestId::from("health-delete"),
                CallerId::from("scope-operator"),
                ProjectId::daemon_scope(),
                IdempotencyKey::from("health-delete-key"),
                "qwen/routed-model",
            )
            .unwrap(),
        ),
        EXCHANGE_TIMEOUT,
    )
    .await
    .unwrap();
    let ResultBody::Failure(failure) = refused.result.body else {
        panic!("Pam must refuse to delete a file it never downloaded");
    };
    assert_eq!(failure.code, FailureCode::InvalidRequest);
    assert!(
        failure
            .message
            .starts_with("Pam did not download this model, so it will not delete the file at"),
        "the refusal must explain itself: {}",
        failure.message
    );
    assert!(
        failure.recovery.as_deref().is_some_and(
            |recovery| recovery.contains("pam model unregister qwen/routed-model --yes")
        ),
        "the refusal must say what the user can do instead: {:?}",
        failure.recovery
    );

    shutdown.send(()).unwrap();
    daemon.await.unwrap().unwrap();

    // Nothing was removed: the registry row the refusal named is still there.
    let store = Store::open(&state_path).unwrap();
    let models = store.list_models().await.unwrap();
    store.shutdown().await.unwrap();
    assert_eq!(models.len(), 1);

    let _ = fs::remove_dir_all(runtime);
}
