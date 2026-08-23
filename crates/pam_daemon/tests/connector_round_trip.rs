use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use pam_core::{CallerCredential, CallerId, GrantId, IdempotencyKey, ProjectId, RequestId};
use pam_daemon::{ConnectorSecretOverride, DaemonConfig, request_exchange, serve_until};
use pam_platform::{LocalEndpoint, SecretBackend, SecretBackendError, SecretLocator};
use pam_policy::{ApprovalRequirement, CapabilityName, Effect, Grant, ResourceScope};
use pam_protocol::{
    ConnectorCredentialAction, ConnectorSecret, ConnectorTestDisposition, FailureCode,
    OperationTruth, RequestEnvelope, ResultBody, ResultPayload, encode,
};
use pam_store::Store;
use tokio::{sync::oneshot, task::JoinHandle};

const TEST_CREDENTIAL: &str = "connector-caller-credential";
const CONNECTOR_SECRET: &str = "ghp_round-trip-connector-secret";
const PROJECT: &str = "project-connectors";

#[derive(Default)]
struct MemorySecretBackend {
    secrets: Mutex<HashMap<String, String>>,
}

impl SecretBackend for MemorySecretBackend {
    fn get(&self, locator: &SecretLocator) -> Result<Option<CallerCredential>, SecretBackendError> {
        Ok(self
            .secrets
            .lock()
            .unwrap()
            .get(locator.as_str())
            .map(CallerCredential::new))
    }

    fn set(
        &self,
        locator: &SecretLocator,
        credential: &CallerCredential,
    ) -> Result<(), SecretBackendError> {
        self.secrets.lock().unwrap().insert(
            locator.as_str().to_owned(),
            credential.expose_secret().to_owned(),
        );
        Ok(())
    }

    fn delete(&self, locator: &SecretLocator) -> Result<bool, SecretBackendError> {
        Ok(self
            .secrets
            .lock()
            .unwrap()
            .remove(locator.as_str())
            .is_some())
    }
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

fn start_daemon(
    endpoint: LocalEndpoint,
    secrets: Arc<MemorySecretBackend>,
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
            connector_secret_backend: Some(ConnectorSecretOverride(secrets as _)),
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

async fn seed_connector_grant(store: &Store, grant_id: &str, caller: &str, capability: &str) {
    store
        .put_grant(pam_store::PutGrant {
            grant: Grant {
                id: GrantId::from(grant_id),
                caller: CallerId::from(caller),
                project: ProjectId::from(PROJECT),
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

fn list_request(request_id: &str) -> RequestEnvelope {
    RequestEnvelope::connector_list(
        RequestId::from(request_id),
        CallerId::from("connector-operator"),
        ProjectId::from(PROJECT),
        IdempotencyKey::new(format!("{request_id}-key")),
    )
    .authenticated(CallerCredential::new(TEST_CREDENTIAL))
}

fn configure_request(
    request_id: &str,
    base_url: Option<String>,
    credential: Option<ConnectorCredentialAction>,
) -> RequestEnvelope {
    RequestEnvelope::connector_configure(
        RequestId::from(request_id),
        CallerId::from("connector-operator"),
        ProjectId::from(PROJECT),
        IdempotencyKey::new(format!("{request_id}-key")),
        "github-actions",
        Some(true),
        base_url,
        credential,
    )
    .unwrap()
    .authenticated(CallerCredential::new(TEST_CREDENTIAL))
}

fn test_request(request_id: &str) -> RequestEnvelope {
    RequestEnvelope::connector_test(
        RequestId::from(request_id),
        CallerId::from("connector-operator"),
        ProjectId::from(PROJECT),
        IdempotencyKey::new(format!("{request_id}-key")),
        "github-actions",
    )
    .unwrap()
    .authenticated(CallerCredential::new(TEST_CREDENTIAL))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(clippy::too_many_lines)] // One fixture proves the complete connector lifecycle boundary.
async fn connector_lifecycle_lists_configures_and_tests_without_exposing_secrets() {
    let runtime = test_runtime("connector-round-trip");
    let endpoint = LocalEndpoint::ipc(runtime.clone());
    let state_path = endpoint.runtime_dir().join("state.sqlite3");

    let seed = Store::open(&state_path).unwrap();
    seed_caller(&seed, "connector-operator", TEST_CREDENTIAL).await;
    seed_connector_grant(
        &seed,
        "configure-grant",
        "connector-operator",
        "connector.configure",
    )
    .await;
    seed_connector_grant(&seed, "test-grant", "connector-operator", "connector.test").await;
    seed.shutdown().await.unwrap();

    let secrets = Arc::new(MemorySecretBackend::default());
    let (shutdown, daemon) = start_daemon(endpoint.clone(), Arc::clone(&secrets));
    wait_until_ready(&endpoint).await;

    // Baseline list: the built-in connector is visible, unconfigured, and
    // credential-free.
    let listed = request_exchange(
        &endpoint,
        &list_request("list-initial"),
        Duration::from_secs(2),
    )
    .await
    .unwrap();
    let ResultBody::Success {
        truth,
        payload: ResultPayload::ConnectorList(initial),
    } = listed.result.body
    else {
        panic!("a baseline connector list read should return a typed result")
    };
    assert_eq!(truth, OperationTruth::Observed);
    assert_eq!(initial.connectors.len(), 2);
    assert_eq!(initial.connectors[0].connector_id, "github-actions");
    assert_eq!(initial.connectors[1].connector_id, "jenkins");
    assert!(!initial.connectors[0].enabled);
    assert!(!initial.connectors[0].credential_present);
    assert!(initial.connectors[0].last_test_status.is_none());

    // Configure with a credential: persisted, acknowledged, and never echoed.
    let configured = request_exchange(
        &endpoint,
        &configure_request(
            "configure",
            Some("https://127.0.0.1:1/api".to_owned()),
            Some(ConnectorCredentialAction::Set {
                secret: ConnectorSecret::new(CONNECTOR_SECRET).unwrap(),
            }),
        ),
        Duration::from_secs(2),
    )
    .await
    .unwrap();
    let encoded = encode(&configured.result).unwrap();
    assert!(
        !encoded
            .windows(CONNECTOR_SECRET.len())
            .any(|window| window == CONNECTOR_SECRET.as_bytes()),
        "configure results must never echo the secret"
    );
    let ResultBody::Success {
        truth,
        payload: ResultPayload::ConnectorConfigure(configured),
    } = configured.result.body
    else {
        panic!("an authorized configure should return a typed result")
    };
    assert_eq!(truth, OperationTruth::Changed);
    assert!(configured.connector.enabled);
    assert!(configured.connector.credential_present);
    assert_eq!(
        configured.connector.base_url.as_deref(),
        Some("https://127.0.0.1:1/api")
    );

    // The self-test runs the real connector against the unroutable base URL and
    // reports a bounded failure without hanging or echoing the secret.
    let tested = request_exchange(&endpoint, &test_request("test"), Duration::from_secs(30))
        .await
        .unwrap();
    let encoded = encode(&tested.result).unwrap();
    assert!(
        !encoded
            .windows(CONNECTOR_SECRET.len())
            .any(|window| window == CONNECTOR_SECRET.as_bytes()),
        "test results must never echo the secret"
    );
    let ResultBody::Success {
        payload: ResultPayload::ConnectorTest(tested),
        ..
    } = tested.result.body
    else {
        panic!("an authorized connector test should return a typed result")
    };
    assert_eq!(tested.connector_id, "github-actions");
    assert_eq!(tested.status, ConnectorTestDisposition::Failed);
    assert!(!tested.detail.is_empty());
    assert!(tested.detail.len() <= 1024);

    // The recorded outcome and stored configuration survive into the listing.
    let listed = request_exchange(
        &endpoint,
        &list_request("list-after"),
        Duration::from_secs(2),
    )
    .await
    .unwrap();
    let ResultBody::Success {
        payload: ResultPayload::ConnectorList(after),
        ..
    } = listed.result.body
    else {
        panic!("connector list reads should return a typed result")
    };
    assert!(after.connectors[0].enabled);
    assert!(after.connectors[0].credential_present);
    assert_eq!(
        after.connectors[0].last_test_status.as_deref(),
        Some("failed")
    );
    assert!(after.connectors[0].last_test_at_ms.is_some());

    // Clearing the credential removes it from the native store.
    let cleared = request_exchange(
        &endpoint,
        &configure_request("clear", None, Some(ConnectorCredentialAction::Clear)),
        Duration::from_secs(2),
    )
    .await
    .unwrap();
    let ResultBody::Success {
        payload: ResultPayload::ConnectorConfigure(cleared),
        ..
    } = cleared.result.body
    else {
        panic!("an authorized credential clear should return a typed result")
    };
    assert!(!cleared.connector.credential_present);
    assert!(secrets.secrets.lock().unwrap().is_empty());

    shutdown.send(()).unwrap();
    daemon.await.unwrap().unwrap();
    let _ = fs::remove_dir_all(runtime);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connector_writes_require_grants_and_list_respects_explicit_deny() {
    let runtime = test_runtime("connector-policy-round-trip");
    let endpoint = LocalEndpoint::ipc(runtime.clone());
    let state_path = endpoint.runtime_dir().join("state.sqlite3");

    let seed = Store::open(&state_path).unwrap();
    seed_caller(&seed, "connector-operator", TEST_CREDENTIAL).await;
    seed_caller(&seed, "denied-observer", "denied-observer-credential").await;
    // connector.list is a baseline capability, so denial coverage needs an
    // explicit deny grant.
    seed.put_grant(pam_store::PutGrant {
        grant: Grant {
            id: GrantId::from("connector-list-denied-grant"),
            caller: CallerId::from("denied-observer"),
            project: ProjectId::from(PROJECT),
            capability: CapabilityName::parse("connector.list").unwrap(),
            resource: ResourceScope::Any,
            effect: Effect::Deny,
            approval: ApprovalRequirement::None,
            expires_at_ms: None,
            revoked_at_ms: None,
        },
        created_at_ms: 2,
    })
    .await
    .unwrap();
    seed.shutdown().await.unwrap();

    let secrets = Arc::new(MemorySecretBackend::default());
    let (shutdown, daemon) = start_daemon(endpoint.clone(), Arc::clone(&secrets));
    wait_until_ready(&endpoint).await;

    // Without grants, configure and test are denied before any credential or
    // network activity.
    for request in [
        configure_request(
            "configure-denied",
            None,
            Some(ConnectorCredentialAction::Set {
                secret: ConnectorSecret::new(CONNECTOR_SECRET).unwrap(),
            }),
        ),
        test_request("test-denied"),
    ] {
        let denied = request_exchange(&endpoint, &request, Duration::from_secs(2))
            .await
            .unwrap();
        assert!(matches!(
            denied.result.body,
            ResultBody::Failure(ref failure) if failure.code == FailureCode::Forbidden
        ));
    }
    assert!(
        secrets.secrets.lock().unwrap().is_empty(),
        "a denied configure must never reach the credential store"
    );

    // An explicit deny grant overrides the baseline list allowance.
    let denied_list = request_exchange(
        &endpoint,
        &RequestEnvelope::connector_list(
            RequestId::from("list-denied"),
            CallerId::from("denied-observer"),
            ProjectId::from(PROJECT),
            IdempotencyKey::from("list-denied-key"),
        )
        .authenticated(CallerCredential::new("denied-observer-credential")),
        Duration::from_secs(2),
    )
    .await
    .unwrap();
    assert!(matches!(
        denied_list.result.body,
        ResultBody::Failure(ref failure) if failure.code == FailureCode::Forbidden
    ));

    shutdown.send(()).unwrap();
    daemon.await.unwrap().unwrap();
    let _ = fs::remove_dir_all(runtime);
}
