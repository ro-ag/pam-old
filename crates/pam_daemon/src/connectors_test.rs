use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use pam_core::CallerCredential;
use pam_platform::{SecretBackend, SecretBackendError, SecretLocator};

use super::connectors::{
    CONFLUENCE, ConnectorRuntime, ConnectorSecretError, GITHUB_ACTIONS, JENKINS, JIRA, SHAREPOINT,
    SONARQUBE, built_in_connector_ids, is_built_in,
};

#[derive(Default)]
pub(crate) struct MemorySecretBackend {
    secrets: Mutex<HashMap<String, String>>,
}

impl SecretBackend for MemorySecretBackend {
    fn get(&self, locator: &SecretLocator) -> Result<Option<CallerCredential>, SecretBackendError> {
        Ok(self
            .secrets
            .lock()
            .expect("secret lock must not be poisoned")
            .get(locator.as_str())
            .map(CallerCredential::new))
    }

    fn set(
        &self,
        locator: &SecretLocator,
        credential: &CallerCredential,
    ) -> Result<(), SecretBackendError> {
        self.secrets
            .lock()
            .expect("secret lock must not be poisoned")
            .insert(
                locator.as_str().to_owned(),
                credential.expose_secret().to_owned(),
            );
        Ok(())
    }

    fn delete(&self, locator: &SecretLocator) -> Result<bool, SecretBackendError> {
        Ok(self
            .secrets
            .lock()
            .expect("secret lock must not be poisoned")
            .remove(locator.as_str())
            .is_some())
    }
}

fn runtime_with_memory_backend() -> (ConnectorRuntime, Arc<MemorySecretBackend>) {
    let backend = Arc::new(MemorySecretBackend::default());
    (
        ConnectorRuntime::new(Some(Arc::clone(&backend) as _)),
        backend,
    )
}

#[test]
fn the_built_in_registry_contains_exactly_github_actions_jenkins_sonarqube_jira_confluence_and_sharepoint()
 {
    assert_eq!(
        built_in_connector_ids(),
        [
            GITHUB_ACTIONS,
            JENKINS,
            SONARQUBE,
            JIRA,
            CONFLUENCE,
            SHAREPOINT
        ]
    );
    assert!(is_built_in("github-actions"));
    assert!(is_built_in("jenkins"));
    assert!(is_built_in("sonarqube"));
    assert!(is_built_in("jira"));
    assert!(is_built_in("confluence"));
    assert!(is_built_in("sharepoint"));
    assert!(!is_built_in("gitlab-ci"));
}

#[tokio::test]
async fn connector_credentials_round_trip_without_appearing_in_debug_output() {
    let (runtime, backend) = runtime_with_memory_backend();

    assert!(!runtime.credential_present(GITHUB_ACTIONS).await.unwrap());
    runtime
        .set_credential(GITHUB_ACTIONS, "ghp_connector-secret".to_owned())
        .await
        .unwrap();
    assert!(runtime.credential_present(GITHUB_ACTIONS).await.unwrap());
    assert_eq!(
        runtime.load_credential(GITHUB_ACTIONS).await.unwrap(),
        Some("ghp_connector-secret".to_owned())
    );
    // The stored key is the opaque connector locator, never the secret's name.
    let stored_keys: Vec<String> = backend.secrets.lock().unwrap().keys().cloned().collect();
    assert!(
        stored_keys
            .iter()
            .all(|key| key.starts_with("pam.connector.v1."))
    );
    assert!(!format!("{runtime:?}").contains("ghp_connector-secret"));

    runtime.clear_credential(GITHUB_ACTIONS).await.unwrap();
    assert!(!runtime.credential_present(GITHUB_ACTIONS).await.unwrap());
    // Clearing an absent credential stays idempotent.
    runtime.clear_credential(GITHUB_ACTIONS).await.unwrap();
}

#[tokio::test]
async fn invalid_connector_secrets_are_rejected_before_reaching_any_backend() {
    let (runtime, backend) = runtime_with_memory_backend();

    for secret in [String::new(), "x".repeat(4097), "line\nbreak".to_owned()] {
        assert_eq!(
            runtime
                .set_credential(GITHUB_ACTIONS, secret)
                .await
                .unwrap_err(),
            ConnectorSecretError::InvalidSecret
        );
    }
    assert!(backend.secrets.lock().unwrap().is_empty());
}
