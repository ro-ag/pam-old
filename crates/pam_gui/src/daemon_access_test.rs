use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use pam_core::{CallerCredential, CallerId, ProjectId};
use pam_policy::{CapabilityName, ResourceName};
use pam_store::{AuthorizationOutcome, AuthorizationRequest, Store};
use uuid::Uuid;

use crate::daemon_access::{read_daemon_access, update_daemon_access};

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("pam-gui-{name}-{}", Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn state_path(&self) -> PathBuf {
        self.0.join("state.sqlite3")
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The commands stamp grants with the real clock, so every assertion has to
/// read policy from a moment after the write it is checking.
fn after_now_ms() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
        + 1
}

/// These fixtures own their own scratch store, so no daemon owns it: the
/// writes stay direct instead of reaching for a real endpoint.
const NO_DAEMON: bool = false;

fn caller() -> CallerId {
    CallerId::from("gui-caller")
}

async fn register_gui_caller(state_path: &std::path::Path) {
    let store = Store::open(state_path).unwrap();
    store
        .register_caller(caller(), CallerCredential::new("gui credential"), 1)
        .await
        .unwrap();
    store.shutdown().await.unwrap();
}

/// What the daemon would decide for a daemon-scope `model.infer` request from
/// the GUI caller right now.
async fn infer_decision(state_path: &std::path::Path) -> AuthorizationOutcome {
    let store = Store::open(state_path).unwrap();
    let outcome = store
        .authorize(
            AuthorizationRequest {
                caller_id: caller(),
                project_id: ProjectId::daemon_scope(),
                capability: CapabilityName::parse("model.infer").unwrap(),
                resource: ResourceName::parse("model:byteshape/qwen3.6-q4ks").unwrap(),
                approval_id: None,
            },
            after_now_ms(),
            100,
        )
        .await
        .unwrap();
    store.shutdown().await.unwrap();
    outcome
}

#[tokio::test]
async fn granting_a_daemon_capability_authorizes_it_and_revoking_returns_it_to_denied() {
    let directory = TestDirectory::new("daemon-access");
    let state_path = directory.state_path();
    register_gui_caller(&state_path).await;

    // Nothing is granted until the owner asks: reading the state writes no grant.
    let initial = read_daemon_access(state_path.clone(), caller())
        .await
        .unwrap();
    assert_eq!(
        initial
            .capabilities
            .iter()
            .map(|row| (row.capability.as_str(), row.granted))
            .collect::<Vec<_>>(),
        vec![
            ("model.infer", false),
            ("model.register", false),
            ("model.unregister", false),
            ("model.verify", false),
            ("model.sweep", false),
            ("model.delete-weights", false),
            ("network.diagnostics", false),
            ("connector.configure", false),
            ("connector.test", false),
            // Reset is tiered, so the danger zone needs one row per tier: a
            // grant for one tier can never be spent on another.
            ("reset.access", false),
            ("reset.identity", false),
            ("reset.history", false),
            ("reset.registry", false),
        ]
    );
    assert_eq!(
        infer_decision(&state_path).await,
        AuthorizationOutcome::Denied
    );

    let granted = update_daemon_access(
        state_path.clone(),
        caller(),
        "model.infer".to_owned(),
        true,
        NO_DAEMON,
    )
    .await
    .unwrap();
    assert_eq!(
        granted
            .capabilities
            .iter()
            .filter(|row| row.granted)
            .map(|row| row.capability.as_str())
            .collect::<Vec<_>>(),
        vec!["model.infer"]
    );
    assert_eq!(
        infer_decision(&state_path).await,
        AuthorizationOutcome::Allowed
    );

    // Granting twice is idempotent, not a second row.
    let again = update_daemon_access(
        state_path.clone(),
        caller(),
        "model.infer".to_owned(),
        true,
        NO_DAEMON,
    )
    .await
    .unwrap();
    assert_eq!(again, granted);
    let store = Store::open(&state_path).unwrap();
    let rows = store
        .active_grants(caller(), ProjectId::daemon_scope(), after_now_ms())
        .await
        .unwrap();
    store.shutdown().await.unwrap();
    assert_eq!(rows.len(), 1);

    let revoked = update_daemon_access(
        state_path.clone(),
        caller(),
        "model.infer".to_owned(),
        false,
        NO_DAEMON,
    )
    .await
    .unwrap();
    assert_eq!(revoked, initial);
    assert_eq!(
        infer_decision(&state_path).await,
        AuthorizationOutcome::Denied
    );
}

#[tokio::test]
async fn a_capability_the_window_does_not_use_is_rejected_before_any_write() {
    let directory = TestDirectory::new("daemon-access-unknown");
    let state_path = directory.state_path();
    register_gui_caller(&state_path).await;

    let error = update_daemon_access(
        state_path.clone(),
        caller(),
        "daemon.stop".to_owned(),
        true,
        NO_DAEMON,
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "This is not a daemon-scoped capability the PAM window uses."
    );

    let store = Store::open(&state_path).unwrap();
    let rows = store
        .active_grants(caller(), ProjectId::daemon_scope(), after_now_ms())
        .await
        .unwrap();
    store.shutdown().await.unwrap();
    assert!(rows.is_empty());
}

#[test]
fn every_surfaced_capability_is_outside_the_policy_baseline() {
    for (capability, _, _) in crate::daemon_access::GUI_DAEMON_CAPABILITIES {
        assert!(
            !pam_policy::BASELINE_CAPABILITIES.contains(&capability),
            "{capability} is baseline: revoking it would not return it to deny"
        );
    }
}
