use std::{
    fs,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use pam_core::{CallerCredential, CallerId, GrantId, IdempotencyKey, ProjectId, RequestId};
use pam_daemon::{DaemonConfig, request_exchange, serve_until};
use pam_platform::LocalEndpoint;
use pam_policy::{ApprovalRequirement, CapabilityName, Effect, Grant, ResourceScope};
use pam_protocol::{FailureCode, OperationTruth, RequestEnvelope, ResultBody, ResultPayload};
use pam_store::Store;
use tokio::{sync::oneshot, task::JoinHandle};

const TEST_CREDENTIAL: &str = "daemon-scope-caller-credential";
/// Connector exchanges touch the native keychain; the security server's
/// first-access code-signature evaluation of a fresh debug binary can take
/// several seconds, so keychain-backed exchanges get a generous deadline.
const KEYCHAIN_EXCHANGE_TIMEOUT: Duration = Duration::from_secs(15);

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
        Duration::from_secs(2),
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
        Duration::from_secs(2),
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
        Duration::from_secs(2),
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
        Duration::from_secs(2),
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
        Duration::from_secs(2),
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
        KEYCHAIN_EXCHANGE_TIMEOUT,
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
        Duration::from_secs(2),
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
        Duration::from_secs(2),
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
        KEYCHAIN_EXCHANGE_TIMEOUT,
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
        KEYCHAIN_EXCHANGE_TIMEOUT,
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
        Duration::from_secs(2),
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
