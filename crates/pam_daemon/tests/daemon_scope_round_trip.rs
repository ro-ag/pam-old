use std::{
    fs,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use pam_client::request_exchange;
use pam_core::{
    CallerCredential, CallerId, ContentDigest, GrantId, IdempotencyKey, ProjectId, RequestId,
};
use pam_daemon::{DaemonConfig, serve_until};
use pam_platform::LocalEndpoint;
use pam_policy::{ApprovalRequirement, CapabilityName, Effect, Grant, ResourceScope};
use pam_protocol::{
    FailureCode, ModelRegistration, OperationTruth, RequestEnvelope, ResultBody, ResultPayload,
};
use pam_store::Store;
use tokio::{sync::oneshot, task::JoinHandle};

const TEST_CREDENTIAL: &str = "daemon-scope-caller-credential";
/// Connector exchanges touch the native keychain; the security server's
/// first-access code-signature evaluation of a fresh debug binary can take
/// several seconds, so keychain-backed exchanges get a generous deadline.
/// How long a test waits for one exchange to come back.
///
/// This is the client's patience, not an assertion: every exchange in this file
/// asserts the *result* it gets, never that the exchange timed out. It has to be
/// generous for two reasons — a shared CI runner can stall a real IPC round trip
/// with `SQLite` behind it, and reading the native trust store is a security-server
/// round trip measured as high as 6s on a developer machine. The rest of the
/// suite already uses 10-15s for the same job; this file used to scatter bare
/// two-second deadlines and flaked on both counts.
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(15);

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

async fn wait_until_ready(endpoint: &LocalEndpoint) {
    for _ in 0..40 {
        if endpoint.socket_path().is_some_and(std::path::Path::exists) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
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
                    .any(|entry| entry.message.contains("PAM daemon ready"))
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
/// sweep sees the dangling row, and deleting the weights of a GGUF PAM only
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
        "a GGUF PAM verified in place is never PAM's to delete"
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
    // refusal says PAM did not download the file plus what to do instead.
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
        panic!("PAM must refuse to delete a file it never downloaded");
    };
    assert_eq!(failure.code, FailureCode::InvalidRequest);
    assert!(
        failure
            .message
            .starts_with("PAM did not download this model, so it will not delete the file at"),
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
