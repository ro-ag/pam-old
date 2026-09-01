use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use pam_client::{request_exchange, request_status};
use pam_core::{
    ApprovalId, CallerCredential, CallerId, GrantId, IdempotencyKey, ProjectId, RequestId,
};
use pam_daemon::{DaemonConfig, serve_until};
use pam_platform::LocalEndpoint;
use pam_policy::{ApprovalRequirement, CapabilityName, Effect, Grant, ResourceName, ResourceScope};
use pam_protocol::{
    ApprovalDecision as ProtocolApprovalDecision, ApprovalDecisionDisposition, Capability,
    FailureCode, MAX_FRAME_SIZE, OperationTruth, PROTOCOL_VERSION, RequestEnvelope, ResultBody,
    ResultPayload, SourceAvailability, encode,
};
use pam_store::{
    AcceptRequest, ApprovalDecision, AuthorizationOutcome, AuthorizationRequest,
    CallerAuthentication, PutGrant, Store, StoreError, TerminalState,
};
use tokio::{sync::oneshot, task::JoinHandle};
use zeromq::{DealerSocket, Socket, SocketSend, ZmqMessage};

const TEST_CREDENTIAL: &str = "integration-caller-credential";

/// How long a test waits for one exchange to come back.
///
/// Client patience, never an assertion: every exchange here asserts the result
/// it gets, not that it timed out, so a generous number costs nothing while the
/// daemon is healthy. It must clear the transport's own budget: the client
/// opens a fresh connection per exchange, and `zeromq`'s `connect_forever`
/// retries a refused or not-yet-listening endpoint on an exponential back-off
/// of roughly 1.4s, 2.0s, 2.7s, 3.8s and 5.3s — **15.15s** in total, which is
/// why the fifteen seconds this file used to spend was the one value certain to
/// fire in the middle of a connect that was about to succeed. Startup is
/// excluded separately, by `wait_until_ready`.
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(45);

/// How long a test waits for the daemon task to finish after it is stopped.
///
/// Patience again, not an assertion: the tests below assert what shutdown left
/// behind, never how quickly it got there. Draining in-flight handlers and
/// closing `SQLite` is real work, and an oversubscribed runner stretches it.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(45);

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn brief_crosses_transport_with_explicit_unavailable_provenance() {
    let runtime = test_runtime("brief-round-trip");
    let endpoint = LocalEndpoint::ipc(runtime.clone());
    let (shutdown, daemon) = start_daemon(endpoint.clone()).await;
    wait_until_ready(&endpoint).await;
    let request = RequestEnvelope::brief(
        RequestId::from("brief-round-trip"),
        CallerId::from("integration-test"),
        ProjectId::from("project-round-trip"),
        IdempotencyKey::from("brief-round-trip"),
    )
    .authenticated(CallerCredential::new(TEST_CREDENTIAL));

    let exchange = request_exchange(&endpoint, &request, EXCHANGE_TIMEOUT)
        .await
        .unwrap();
    assert!(exchange.events.is_empty());
    let ResultBody::Success {
        truth,
        payload: ResultPayload::Brief(brief),
    } = exchange.result.body
    else {
        panic!("brief should return a typed result")
    };
    assert_eq!(brief.provenance.len(), 1);
    assert_eq!(
        brief.provenance[0].availability,
        SourceAvailability::Unavailable
    );
    assert_eq!(truth, OperationTruth::Unresolved);

    shutdown.send(()).unwrap();
    daemon.await.unwrap().unwrap();
    let _ = fs::remove_dir_all(runtime);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn network_diagnostics_require_an_authenticated_project_grant() {
    let runtime = test_runtime("network-policy");
    let endpoint = LocalEndpoint::ipc(runtime.clone());
    let state_path = endpoint.runtime_dir().join("state.sqlite3");
    let (shutdown, daemon) = start_daemon(endpoint.clone()).await;
    wait_until_ready(&endpoint).await;
    let request = |suffix: &str| {
        RequestEnvelope::network_diagnostics(
            RequestId::new(format!("network-{suffix}")),
            CallerId::from("integration-test"),
            ProjectId::from("project-round-trip"),
            IdempotencyKey::new(format!("network-{suffix}")),
        )
        .authenticated(CallerCredential::new(TEST_CREDENTIAL))
    };

    let denied = request_exchange(&endpoint, &request("denied"), EXCHANGE_TIMEOUT)
        .await
        .unwrap();
    assert!(matches!(
        denied.result.body,
        ResultBody::Failure(ref failure) if failure.code == FailureCode::Forbidden
    ));

    let store = Store::open(&state_path).unwrap();
    store
        .put_grant(PutGrant {
            grant: Grant {
                id: GrantId::from("integration-network-diagnostics"),
                caller: CallerId::from("integration-test"),
                project: ProjectId::from("project-round-trip"),
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
    store.shutdown().await.unwrap();

    let allowed = request_exchange(&endpoint, &request("allowed"), EXCHANGE_TIMEOUT)
        .await
        .unwrap();
    assert!(matches!(
        allowed.result.body,
        ResultBody::Success {
            payload: ResultPayload::NetworkDiagnostics(_),
            ..
        }
    ));

    shutdown.send(()).unwrap();
    daemon.await.unwrap().unwrap();
    let _ = fs::remove_dir_all(runtime);
}

fn status_request() -> RequestEnvelope {
    RequestEnvelope::status(
        RequestId::from("request-round-trip"),
        CallerId::from("integration-test"),
        ProjectId::from("project-round-trip"),
        IdempotencyKey::from("status-round-trip"),
    )
    .authenticated(CallerCredential::new(TEST_CREDENTIAL))
}

fn stop_request(request_id: &str, caller_id: &str, credential: Option<&str>) -> RequestEnvelope {
    let request = RequestEnvelope::stop(
        RequestId::from(request_id),
        CallerId::from(caller_id),
        ProjectId::from("project-round-trip"),
        IdempotencyKey::new(format!("{request_id}-key")),
    );
    credential.map_or(request.clone(), |credential| {
        request.authenticated(CallerCredential::new(credential))
    })
}

fn approval_status(request_id: &str, approval_id: Option<ApprovalId>) -> RequestEnvelope {
    let request = RequestEnvelope::status(
        RequestId::from(request_id),
        CallerId::from("approval-caller"),
        ProjectId::from("project-round-trip"),
        IdempotencyKey::new(format!("{request_id}-key")),
    )
    .authenticated(CallerCredential::new("approval-caller-credential"));
    match approval_id {
        Some(approval_id) => request.with_approval(approval_id),
        None => request,
    }
}

fn approval_project_current(request_id: &str, approval_id: Option<ApprovalId>) -> RequestEnvelope {
    let request = RequestEnvelope::project_current(
        RequestId::from(request_id),
        CallerId::from("approval-caller"),
        ProjectId::from("project-round-trip"),
        IdempotencyKey::new(format!("{request_id}-key")),
    )
    .authenticated(CallerCredential::new("approval-caller-credential"));
    match approval_id {
        Some(approval_id) => request.with_approval(approval_id),
        None => request,
    }
}

fn approval_decision_request(
    request_id: &str,
    project_id: &str,
    caller_id: &str,
    credential: Option<&str>,
    approval_id: ApprovalId,
    decision: ProtocolApprovalDecision,
) -> RequestEnvelope {
    let request = RequestEnvelope::approval_decide(
        RequestId::from(request_id),
        CallerId::from(caller_id),
        ProjectId::from(project_id),
        IdempotencyKey::new(format!("{request_id}-key")),
        approval_id,
        decision,
    );
    credential.map_or(request.clone(), |credential| {
        request.authenticated(CallerCredential::new(credential))
    })
}

async fn approve_challenge(state_path: &std::path::Path, body: ResultBody) -> ApprovalId {
    let ResultBody::Failure(failure) = body else {
        panic!("approval-gated capability should return a challenge")
    };
    assert_eq!(failure.code, FailureCode::ApprovalRequired);
    let approval_id = failure
        .approval
        .expect("typed approval challenge")
        .approval_id;
    let store = Store::open(state_path).unwrap();
    store
        .decide_approval(
            approval_id.clone(),
            CallerId::from("integration-test"),
            ApprovalDecision::Approve,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis()
                .try_into()
                .unwrap(),
        )
        .await
        .unwrap();
    store.shutdown().await.unwrap();
    approval_id
}

async fn seed_approval_caller(state_path: &std::path::Path) {
    let seed = Store::open(state_path).unwrap();
    seed.register_caller(
        CallerId::from("approval-caller"),
        CallerCredential::new("approval-caller-credential"),
        1,
    )
    .await
    .unwrap();
    for capability in [
        "daemon.status",
        "daemon.stop",
        "project.current",
        "brief.read",
    ] {
        seed.put_grant(PutGrant {
            grant: Grant {
                id: GrantId::new(format!("approval-{capability}")),
                caller: CallerId::from("approval-caller"),
                project: ProjectId::from("project-round-trip"),
                capability: CapabilityName::parse(capability).unwrap(),
                resource: if capability == "project.current" {
                    ResourceScope::Exact(ResourceName::parse("project").unwrap())
                } else {
                    ResourceScope::Any
                },
                effect: Effect::Allow,
                approval: ApprovalRequirement::Once,
                expires_at_ms: None,
                revoked_at_ms: None,
            },
            created_at_ms: 1,
        })
        .await
        .unwrap();
    }
    seed.shutdown().await.unwrap();
}

async fn start_daemon(
    endpoint: LocalEndpoint,
) -> (
    oneshot::Sender<()>,
    JoinHandle<Result<(), pam_daemon::DaemonError>>,
) {
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let state_path = endpoint.runtime_dir().join("state.sqlite3");
    let store = Store::open(&state_path).unwrap();
    let caller_id = CallerId::from("integration-test");
    let credential = CallerCredential::new(TEST_CREDENTIAL);
    if store
        .authenticate_caller(caller_id.clone(), credential.clone())
        .await
        .unwrap()
        == CallerAuthentication::UnknownCaller
    {
        store
            .register_caller(caller_id, credential, 1)
            .await
            .unwrap();
    }
    for capability in ["daemon.status", "daemon.stop", "brief.read"] {
        let result = store
            .put_grant(PutGrant {
                grant: Grant {
                    id: GrantId::new(format!("integration-{capability}")),
                    caller: CallerId::from("integration-test"),
                    project: ProjectId::from("project-round-trip"),
                    capability: CapabilityName::parse(capability).unwrap(),
                    resource: ResourceScope::Any,
                    effect: Effect::Allow,
                    approval: ApprovalRequirement::None,
                    expires_at_ms: None,
                    revoked_at_ms: None,
                },
                created_at_ms: 1,
            })
            .await;
        assert!(
            result.is_ok() || matches!(result, Err(StoreError::GrantAlreadyExists(_))),
            "integration grant should be present"
        );
    }
    store.shutdown().await.unwrap();
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_crosses_transport_and_returns_an_immediate_result() {
    let runtime = test_runtime("round-trip");
    let endpoint = LocalEndpoint::ipc(runtime.clone());
    let (shutdown, daemon) = start_daemon(endpoint.clone()).await;

    wait_until_ready(&endpoint).await;

    let mut malformed_client = DealerSocket::new();
    malformed_client.connect(endpoint.address()).await.unwrap();
    let mut multipart = ZmqMessage::from(vec![1]);
    multipart.push_back(vec![2].into());
    malformed_client.send(multipart).await.unwrap();
    malformed_client
        .send(vec![0; MAX_FRAME_SIZE + 1].into())
        .await
        .unwrap();

    let request = status_request();
    let exchange = request_status(&endpoint, &request, EXCHANGE_TIMEOUT)
        .await
        .unwrap();

    assert_eq!(exchange.result.request_id, request.request_id);
    assert_eq!(exchange.result.project_id, request.project_id);
    assert!(exchange.events.is_empty());

    let ResultBody::Success { truth, payload } = exchange.result.body else {
        panic!("status should succeed")
    };
    assert_eq!(truth, OperationTruth::Observed);
    let ResultPayload::Status(status) = payload else {
        panic!("status should return a status payload")
    };
    assert!(status.ready);
    assert!(status.healthy);
    assert_eq!(status.queue_depth, 0);

    let mut future_request = status_request();
    future_request.protocol_version = PROTOCOL_VERSION + 1;
    let future_exchange = request_status(&endpoint, &future_request, EXCHANGE_TIMEOUT)
        .await
        .unwrap();
    assert!(future_exchange.events.is_empty());
    let ResultBody::Failure(failure) = future_exchange.result.body else {
        panic!("future protocol request should receive a typed failure")
    };
    assert_eq!(failure.code, FailureCode::UnsupportedProtocolVersion);

    shutdown.send(()).unwrap();
    tokio::time::timeout(SHUTDOWN_TIMEOUT, daemon)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(endpoint.ownership_path().exists());
    assert!(endpoint.socket_path().is_none_or(|path| !path.exists()));

    let (second_shutdown, second_daemon) = start_daemon(endpoint.clone()).await;
    wait_until_ready(&endpoint).await;
    request_status(&endpoint, &status_request(), EXCHANGE_TIMEOUT)
        .await
        .unwrap();
    second_shutdown.send(()).unwrap();
    second_daemon.await.unwrap().unwrap();
    let _ = fs::remove_dir_all(runtime);
}

#[allow(clippy::too_many_lines)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_denies_unauthorized_callers_acknowledges_before_teardown_and_allows_restart() {
    let runtime = test_runtime("stop-round-trip");
    let endpoint = LocalEndpoint::ipc(runtime.clone());
    let state_path = endpoint.runtime_dir().join("state.sqlite3");
    let (shutdown, daemon) = start_daemon(endpoint.clone()).await;
    wait_until_ready(&endpoint).await;

    let store = Store::open(&state_path).unwrap();
    store
        .register_caller(
            CallerId::from("stop-denied-caller"),
            CallerCredential::new("stop-denied-credential"),
            2,
        )
        .await
        .unwrap();
    // daemon.stop is a baseline capability, so denial coverage needs an
    // explicit deny grant.
    store
        .put_grant(PutGrant {
            grant: Grant {
                id: GrantId::from("stop-denied-grant"),
                caller: CallerId::from("stop-denied-caller"),
                project: ProjectId::from("project-round-trip"),
                capability: CapabilityName::parse("daemon.stop").unwrap(),
                resource: ResourceScope::Any,
                effect: Effect::Deny,
                approval: ApprovalRequirement::None,
                expires_at_ms: None,
                revoked_at_ms: None,
            },
            created_at_ms: 3,
        })
        .await
        .unwrap();
    store.shutdown().await.unwrap();

    for (request, expected) in [
        (
            stop_request("stop-unauthenticated", "integration-test", None),
            FailureCode::Unauthenticated,
        ),
        (
            stop_request(
                "stop-policy-denied",
                "stop-denied-caller",
                Some("stop-denied-credential"),
            ),
            FailureCode::Forbidden,
        ),
    ] {
        let exchange = request_exchange(&endpoint, &request, EXCHANGE_TIMEOUT)
            .await
            .unwrap();
        assert!(matches!(
            exchange.result.body,
            ResultBody::Failure(ref failure) if failure.code == expected
        ));
        assert!(!daemon.is_finished());
        assert!(endpoint.socket_path().is_some_and(std::path::Path::exists));
    }

    let stop = stop_request("stop-allowed", "integration-test", Some(TEST_CREDENTIAL));
    let acknowledged = request_exchange(&endpoint, &stop, EXCHANGE_TIMEOUT)
        .await
        .unwrap();
    assert_eq!(acknowledged.result.request_id, stop.request_id);
    assert_eq!(acknowledged.result.project_id, stop.project_id);
    assert!(matches!(
        acknowledged.result.body,
        ResultBody::Success {
            truth: OperationTruth::Changed,
            payload: ResultPayload::DaemonLifecycle(ref result),
        } if result.stopping
    ));

    drop(shutdown);
    tokio::time::timeout(SHUTDOWN_TIMEOUT, daemon)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(endpoint.ownership_path().exists());
    assert!(endpoint.socket_path().is_none_or(|path| !path.exists()));

    let (second_shutdown, second_daemon) = start_daemon(endpoint.clone()).await;
    wait_until_ready(&endpoint).await;
    request_status(&endpoint, &status_request(), EXCHANGE_TIMEOUT)
        .await
        .unwrap();
    request_exchange(
        &endpoint,
        &stop_request(
            "stop-after-restart",
            "integration-test",
            Some(TEST_CREDENTIAL),
        ),
        EXCHANGE_TIMEOUT,
    )
    .await
    .unwrap();
    drop(second_shutdown);
    second_daemon.await.unwrap().unwrap();
    let _ = fs::remove_dir_all(runtime);
}

#[tokio::test]
async fn unavailable_daemon_returns_recovery_without_auto_start() {
    let runtime = test_runtime("unavailable");
    let endpoint = LocalEndpoint::ipc(runtime.clone());

    let error = request_status(&endpoint, &status_request(), Duration::from_millis(50))
        .await
        .unwrap_err();

    assert!(error.is_unavailable());
    assert_eq!(error.recovery_action(), Some("pam daemon"));
    assert!(!endpoint.ownership_path().exists());
    assert!(!endpoint.socket_path().unwrap().exists());
    let _ = fs::remove_dir_all(runtime);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authentication_rejects_missing_wrong_and_revoked_credentials() {
    let runtime = test_runtime("authentication");
    let endpoint = LocalEndpoint::ipc(runtime.clone());
    let state_path = endpoint.runtime_dir().join("state.sqlite3");
    let (shutdown, daemon) = start_daemon(endpoint.clone()).await;
    wait_until_ready(&endpoint).await;

    let missing = RequestEnvelope::status(
        RequestId::from("auth-missing"),
        CallerId::from("integration-test"),
        ProjectId::from("project-round-trip"),
        IdempotencyKey::from("auth-missing"),
    );
    let wrong = RequestEnvelope::status(
        RequestId::from("auth-wrong"),
        CallerId::from("integration-test"),
        ProjectId::from("project-round-trip"),
        IdempotencyKey::from("auth-wrong"),
    )
    .authenticated(CallerCredential::new("wrong credential"));

    let missing_failure = request_status(&endpoint, &missing, EXCHANGE_TIMEOUT)
        .await
        .unwrap()
        .result;
    let wrong_failure = request_status(&endpoint, &wrong, EXCHANGE_TIMEOUT)
        .await
        .unwrap()
        .result;
    for result in [missing_failure, wrong_failure] {
        let ResultBody::Failure(failure) = result.body else {
            panic!("unauthenticated request should fail")
        };
        assert_eq!(failure.code, FailureCode::Unauthenticated);
        assert_eq!(failure.message, "caller authentication failed");
        assert_eq!(failure.recovery.as_deref(), Some("pam caller register"));
    }

    let valid = status_request();
    assert!(matches!(
        request_status(&endpoint, &valid, EXCHANGE_TIMEOUT)
            .await
            .unwrap()
            .result
            .body,
        ResultBody::Success { .. }
    ));

    let store = Store::open(&state_path).unwrap();
    assert_eq!(
        store
            .revoke_caller(CallerId::from("integration-test"), 2)
            .await
            .unwrap(),
        pam_store::CallerRevocation::Revoked
    );
    store.shutdown().await.unwrap();
    let revoked = RequestEnvelope::status(
        RequestId::from("auth-revoked"),
        CallerId::from("integration-test"),
        ProjectId::from("project-round-trip"),
        IdempotencyKey::from("auth-revoked"),
    )
    .authenticated(CallerCredential::new(TEST_CREDENTIAL));
    let revoked_result = request_status(&endpoint, &revoked, EXCHANGE_TIMEOUT)
        .await
        .unwrap()
        .result;
    let ResultBody::Failure(failure) = revoked_result.body else {
        panic!("revoked caller should fail")
    };
    assert_eq!(failure.code, FailureCode::Unauthenticated);
    assert_eq!(failure.message, "caller authentication failed");

    shutdown.send(()).unwrap();
    daemon.await.unwrap().unwrap();
    let _ = fs::remove_dir_all(runtime);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::too_many_lines)] // One transport fixture proves the complete approval-decision boundary.
async fn project_current_and_remote_approval_decisions_are_scoped_and_fail_closed() {
    let runtime = test_runtime("project-current-approval");
    let endpoint = LocalEndpoint::ipc(runtime.clone());
    let state_path = endpoint.runtime_dir().join("state.sqlite3");
    seed_approval_caller(&state_path).await;

    let seed = Store::open(&state_path).unwrap();
    seed.register_caller(
        CallerId::from("revoked-reviewer"),
        CallerCredential::new("revoked-reviewer-credential"),
        2,
    )
    .await
    .unwrap();
    seed.revoke_caller(CallerId::from("revoked-reviewer"), 3)
        .await
        .unwrap();
    seed.accept(
        AcceptRequest {
            request_id: RequestId::from("current-terminal"),
            caller_id: CallerId::from("approval-caller"),
            project_id: ProjectId::from("project-round-trip"),
            idempotency_key: IdempotencyKey::from("current-terminal-key"),
            operation_kind: "test.operation".to_owned(),
            operation: b"operation-blob-secret".to_vec(),
        },
        4,
    )
    .await
    .unwrap();
    let terminal = seed.claim("seed-worker", 5, 100).await.unwrap().unwrap();
    seed.finish(
        terminal.lease,
        6,
        TerminalState::Succeeded,
        b"result-blob-secret".to_vec(),
    )
    .await
    .unwrap();
    seed.shutdown().await.unwrap();

    let (shutdown, daemon) = start_daemon(endpoint.clone()).await;
    wait_until_ready(&endpoint).await;

    // project.current is a baseline read: an authenticated caller with no
    // matching grant is served instead of rejected. Explicit deny and
    // approval-required grants (exercised below) still take over.
    let ungranted = RequestEnvelope::project_current(
        RequestId::from("current-ungranted"),
        CallerId::from("integration-test"),
        ProjectId::from("project-round-trip"),
        IdempotencyKey::from("current-ungranted-key"),
    )
    .authenticated(CallerCredential::new(TEST_CREDENTIAL));
    assert!(matches!(
        request_exchange(&endpoint, &ungranted, EXCHANGE_TIMEOUT)
            .await
            .unwrap()
            .result
            .body,
        ResultBody::Success { .. }
    ));

    let current = approval_project_current("current-challenge", None);
    let challenged = request_exchange(&endpoint, &current, EXCHANGE_TIMEOUT)
        .await
        .unwrap();
    assert!(challenged.events.is_empty());
    let ResultBody::Failure(challenge_failure) = challenged.result.body else {
        panic!("project.current should require its configured one-time approval")
    };
    assert_eq!(challenge_failure.code, FailureCode::ApprovalRequired);
    let approval_id = challenge_failure.approval.unwrap().approval_id;

    for unauthenticated in [
        approval_decision_request(
            "decision-missing",
            "project-round-trip",
            "approval-caller",
            None,
            approval_id.clone(),
            ProtocolApprovalDecision::Approve,
        ),
        approval_decision_request(
            "decision-wrong-caller",
            "project-round-trip",
            "other-caller",
            Some("approval-caller-credential"),
            approval_id.clone(),
            ProtocolApprovalDecision::Approve,
        ),
        approval_decision_request(
            "decision-revoked",
            "project-round-trip",
            "revoked-reviewer",
            Some("revoked-reviewer-credential"),
            approval_id.clone(),
            ProtocolApprovalDecision::Approve,
        ),
    ] {
        assert!(matches!(
            request_exchange(&endpoint, &unauthenticated, EXCHANGE_TIMEOUT)
                .await
                .unwrap()
                .result
                .body,
            ResultBody::Failure(ref failure) if failure.code == FailureCode::Unauthenticated
        ));
    }

    let wrong_project = approval_decision_request(
        "decision-wrong-project",
        "other-project",
        "approval-caller",
        Some("approval-caller-credential"),
        approval_id.clone(),
        ProtocolApprovalDecision::Approve,
    );
    let wrong_project_result = request_exchange(&endpoint, &wrong_project, EXCHANGE_TIMEOUT)
        .await
        .unwrap()
        .result;
    let ResultBody::Failure(wrong_project_failure) = wrong_project_result.body else {
        panic!("a project-mismatched approval decision must fail")
    };
    assert_eq!(wrong_project_failure.code, FailureCode::Forbidden);
    assert_eq!(
        wrong_project_failure.message,
        "approval is unavailable for this project or caller"
    );
    assert!(!wrong_project_failure.message.contains(approval_id.as_str()));

    for (request_id, decision) in [
        (
            "decision-other-caller-approve",
            ProtocolApprovalDecision::Approve,
        ),
        ("decision-other-caller-deny", ProtocolApprovalDecision::Deny),
    ] {
        let other_caller = approval_decision_request(
            request_id,
            "project-round-trip",
            "integration-test",
            Some(TEST_CREDENTIAL),
            approval_id.clone(),
            decision,
        );
        let result = request_exchange(&endpoint, &other_caller, EXCHANGE_TIMEOUT)
            .await
            .unwrap()
            .result;
        let ResultBody::Failure(failure) = result.body else {
            panic!("another active caller must not decide the requester's approval")
        };
        assert_eq!(failure.code, FailureCode::Forbidden);
        assert_eq!(
            failure.message,
            "approval is unavailable for this project or caller"
        );
        assert!(!failure.message.contains(approval_id.as_str()));
    }

    let malformed = approval_decision_request(
        "decision-receipt-shaped",
        "project-round-trip",
        "approval-caller",
        Some("approval-caller-credential"),
        approval_id.clone(),
        ProtocolApprovalDecision::Approve,
    )
    .with_approval(ApprovalId::from("unexpected-receipt"));
    assert!(matches!(
        request_exchange(&endpoint, &malformed, EXCHANGE_TIMEOUT)
            .await
            .unwrap()
            .result
            .body,
        ResultBody::Failure(ref failure) if failure.code == FailureCode::InvalidRequest
    ));
    let oversized_approval_id = approval_decision_request(
        "decision-oversized-id",
        "project-round-trip",
        "approval-caller",
        Some("approval-caller-credential"),
        ApprovalId::from("x".repeat(257)),
        ProtocolApprovalDecision::Approve,
    );
    assert!(matches!(
        request_exchange(
            &endpoint,
            &oversized_approval_id,
            EXCHANGE_TIMEOUT,
        )
        .await
        .unwrap()
        .result
        .body,
        ResultBody::Failure(ref failure) if failure.code == FailureCode::InvalidRequest
    ));

    let approve = approval_decision_request(
        "decision-approve",
        "project-round-trip",
        "approval-caller",
        Some("approval-caller-credential"),
        approval_id.clone(),
        ProtocolApprovalDecision::Approve,
    );
    let approved = request_exchange(&endpoint, &approve, EXCHANGE_TIMEOUT)
        .await
        .unwrap();
    assert!(approved.events.is_empty());
    assert!(matches!(
        approved.result.body,
        ResultBody::Success {
            truth: OperationTruth::Changed,
            payload: ResultPayload::ApprovalDecision(ref result),
        } if result.approval_id == approval_id
            && result.disposition == ApprovalDecisionDisposition::Approved
    ));
    let approved_retry = request_exchange(
        &endpoint,
        &approval_decision_request(
            "decision-approve-retry",
            "project-round-trip",
            "approval-caller",
            Some("approval-caller-credential"),
            approval_id.clone(),
            ProtocolApprovalDecision::Approve,
        ),
        EXCHANGE_TIMEOUT,
    )
    .await
    .unwrap();
    assert!(matches!(
        approved_retry.result.body,
        ResultBody::Success {
            payload: ResultPayload::ApprovalDecision(ref result),
            ..
        } if result.approval_id == approval_id
            && result.disposition == ApprovalDecisionDisposition::Approved
    ));
    assert!(matches!(
        request_exchange(
            &endpoint,
            &approval_decision_request(
                "decision-approve-conflict",
                "project-round-trip",
                "approval-caller",
                Some("approval-caller-credential"),
                approval_id.clone(),
                ProtocolApprovalDecision::Deny,
            ),
            EXCHANGE_TIMEOUT,
        )
        .await
        .unwrap()
        .result
        .body,
        ResultBody::Failure(ref failure) if failure.code == FailureCode::Forbidden
    ));

    let exact_retry = approval_project_current("current-approved", Some(approval_id.clone()));
    let current_result = request_exchange(&endpoint, &exact_retry, EXCHANGE_TIMEOUT)
        .await
        .unwrap();
    assert!(current_result.events.is_empty());
    let encoded_current = encode(&current_result.result).unwrap();
    assert!(
        !encoded_current
            .windows(b"operation-blob-secret".len())
            .any(|window| window == b"operation-blob-secret")
    );
    assert!(
        !encoded_current
            .windows(b"result-blob-secret".len())
            .any(|window| window == b"result-blob-secret")
    );
    assert!(matches!(
        current_result.result.body,
        ResultBody::Success {
            truth: OperationTruth::Observed,
            payload: ResultPayload::ProjectCurrent(ref current),
        } if current.latest.as_ref().is_some_and(|latest|
            latest.request_id.as_str() == "current-terminal"
                && latest.operation_kind() == "test.operation")
    ));
    let store = Store::open(&state_path).unwrap();
    assert!(matches!(
        store.snapshot(exact_retry.request_id.clone()).await,
        Err(StoreError::RequestNotFound(_))
    ));
    store.shutdown().await.unwrap();
    assert!(matches!(
        request_exchange(
            &endpoint,
            &approval_project_current("current-reused", Some(approval_id)),
            EXCHANGE_TIMEOUT,
        )
        .await
        .unwrap()
        .result
        .body,
        ResultBody::Failure(ref failure) if failure.code == FailureCode::Forbidden
    ));

    let denied_challenge = request_exchange(
        &endpoint,
        &approval_project_current("current-deny-challenge", None),
        EXCHANGE_TIMEOUT,
    )
    .await
    .unwrap();
    let ResultBody::Failure(denied_failure) = denied_challenge.result.body else {
        panic!("a fresh project current request should create a fresh challenge")
    };
    let denied_id = denied_failure.approval.unwrap().approval_id;
    let denied = request_exchange(
        &endpoint,
        &approval_decision_request(
            "decision-deny",
            "project-round-trip",
            "approval-caller",
            Some("approval-caller-credential"),
            denied_id.clone(),
            ProtocolApprovalDecision::Deny,
        ),
        EXCHANGE_TIMEOUT,
    )
    .await
    .unwrap();
    assert!(matches!(
        denied.result.body,
        ResultBody::Success {
            payload: ResultPayload::ApprovalDecision(ref result),
            ..
        } if result.approval_id == denied_id
            && result.disposition == ApprovalDecisionDisposition::Denied
    ));
    assert!(matches!(
        request_exchange(
            &endpoint,
            &approval_decision_request(
                "decision-deny-retry",
                "project-round-trip",
                "approval-caller",
                Some("approval-caller-credential"),
                denied_id.clone(),
                ProtocolApprovalDecision::Deny,
            ),
            EXCHANGE_TIMEOUT,
        )
        .await
        .unwrap()
        .result
        .body,
        ResultBody::Success {
            payload: ResultPayload::ApprovalDecision(ref result),
            ..
        } if result.approval_id == denied_id
            && result.disposition == ApprovalDecisionDisposition::Denied
    ));

    let store = Store::open(&state_path).unwrap();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis()
        .try_into()
        .unwrap();
    let AuthorizationOutcome::ApprovalRequired {
        approval_id: expired_id,
        ..
    } = store
        .authorize(
            AuthorizationRequest {
                caller_id: CallerId::from("approval-caller"),
                project_id: ProjectId::from("project-round-trip"),
                capability: CapabilityName::parse("project.current").unwrap(),
                resource: ResourceName::parse("project").unwrap(),
                approval_id: None,
            },
            now,
            1,
        )
        .await
        .unwrap()
    else {
        panic!("the approval-required grant should create an expiring challenge")
    };
    store.shutdown().await.unwrap();
    tokio::time::sleep(Duration::from_millis(2)).await;
    let expired = request_exchange(
        &endpoint,
        &approval_decision_request(
            "decision-expired",
            "project-round-trip",
            "approval-caller",
            Some("approval-caller-credential"),
            expired_id.clone(),
            ProtocolApprovalDecision::Approve,
        ),
        EXCHANGE_TIMEOUT,
    )
    .await
    .unwrap();
    assert!(matches!(
        expired.result.body,
        ResultBody::Success {
            payload: ResultPayload::ApprovalDecision(ref result),
            ..
        } if result.approval_id == expired_id
            && result.disposition == ApprovalDecisionDisposition::Expired
    ));
    assert!(matches!(
        request_exchange(
            &endpoint,
            &approval_decision_request(
                "decision-expired-retry",
                "project-round-trip",
                "approval-caller",
                Some("approval-caller-credential"),
                expired_id.clone(),
                ProtocolApprovalDecision::Deny,
            ),
            EXCHANGE_TIMEOUT,
        )
        .await
        .unwrap()
        .result
        .body,
        ResultBody::Success {
            payload: ResultPayload::ApprovalDecision(ref result),
            ..
        } if result.approval_id == expired_id
            && result.disposition == ApprovalDecisionDisposition::Expired
    ));

    request_exchange(
        &endpoint,
        &stop_request(
            "stop-current-approval-test",
            "integration-test",
            Some(TEST_CREDENTIAL),
        ),
        EXCHANGE_TIMEOUT,
    )
    .await
    .unwrap();
    drop(shutdown);
    daemon.await.unwrap().unwrap();
    let _ = fs::remove_dir_all(runtime);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exact_approval_is_required_bound_to_effect_and_consumed_once() {
    let runtime = test_runtime("policy-approval");
    let endpoint = LocalEndpoint::ipc(runtime.clone());
    let state_path = endpoint.runtime_dir().join("state.sqlite3");
    seed_approval_caller(&state_path).await;

    let (shutdown, daemon) = start_daemon(endpoint.clone()).await;
    wait_until_ready(&endpoint).await;

    let challenge_result = request_status(
        &endpoint,
        &approval_status("approval-request", None),
        EXCHANGE_TIMEOUT,
    )
    .await
    .unwrap()
    .result;
    let challenge = approve_challenge(&state_path, challenge_result.body).await;

    let wrong_effect = RequestEnvelope::brief(
        RequestId::from("approval-wrong-effect"),
        CallerId::from("approval-caller"),
        ProjectId::from("project-round-trip"),
        IdempotencyKey::from("approval-wrong-effect-key"),
    )
    .authenticated(CallerCredential::new("approval-caller-credential"))
    .with_approval(challenge.clone());
    let wrong = request_exchange(&endpoint, &wrong_effect, EXCHANGE_TIMEOUT)
        .await
        .unwrap();
    assert!(matches!(
        wrong.result.body,
        ResultBody::Failure(ref failure) if failure.code == FailureCode::Forbidden
    ));

    let approved = approval_status("approval-approved", Some(challenge.clone()));
    assert!(matches!(
        request_status(&endpoint, &approved, EXCHANGE_TIMEOUT)
            .await
            .unwrap()
            .result
            .body,
        ResultBody::Success { .. }
    ));
    let replay = approval_status("approval-replay", Some(challenge));
    assert!(matches!(
        request_status(&endpoint, &replay, EXCHANGE_TIMEOUT)
            .await
            .unwrap()
            .result
            .body,
        ResultBody::Failure(ref failure) if failure.code == FailureCode::Forbidden
    ));

    let stop = stop_request(
        "approval-stop",
        "approval-caller",
        Some("approval-caller-credential"),
    );
    let stop_challenge = request_exchange(&endpoint, &stop, EXCHANGE_TIMEOUT)
        .await
        .unwrap();
    let stop_approval = approve_challenge(&state_path, stop_challenge.result.body).await;
    let stopped = request_exchange(
        &endpoint,
        &stop.with_approval(stop_approval),
        EXCHANGE_TIMEOUT,
    )
    .await
    .unwrap();
    assert!(matches!(
        stopped.result.body,
        ResultBody::Success {
            payload: ResultPayload::DaemonLifecycle(_),
            ..
        }
    ));

    drop(shutdown);
    daemon.await.unwrap().unwrap();
    let _ = fs::remove_dir_all(runtime);
}
