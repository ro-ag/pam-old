//! The daemon's built-in connector registry and its credential seam.
//!
//! Connector configuration lives in the durable store; credentials live only
//! in the operating system's native credential store under a connector-specific
//! locator domain. Credential values never appear in logs, results, or errors.

use std::{fmt, sync::Arc};

use pam_connectors::github::{GitHubActions, GitHubTransport, ReqwestGitHubTransport};
use pam_core::CallerCredential;
use pam_platform::{NativeSecretBackend, SecretBackend, SecretBackendError, SecretLocator};

pub(crate) const GITHUB_ACTIONS: &str = "github-actions";
pub(crate) const GITHUB_DEFAULT_API_BASE: &str = "https://api.github.com/";
pub(crate) const MAX_CONNECTOR_SECRET_BYTES: usize = 4096;

/// The flow-step capability names executable through the GitHub connector.
pub(crate) const GITHUB_DISCOVER_CAPABILITY: &str = "runs.discover-failed";
pub(crate) const GITHUB_COLLECT_LOGS_CAPABILITY: &str = "runs.collect-logs";

#[must_use]
pub(crate) fn built_in_connector_ids() -> [&'static str; 1] {
    [GITHUB_ACTIONS]
}

#[must_use]
pub(crate) fn is_built_in(connector_id: &str) -> bool {
    built_in_connector_ids().contains(&connector_id)
}

/// Sanitized connector credential-store failure; carries no secret material.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ConnectorSecretError {
    InvalidSecret,
    Unavailable,
    Denied,
}

impl ConnectorSecretError {
    pub(crate) const fn message(self) -> &'static str {
        match self {
            Self::InvalidSecret => "connector credential is invalid",
            Self::Unavailable => "the native credential store is unavailable",
            Self::Denied => "access to the native credential store was denied",
        }
    }
}

impl From<SecretBackendError> for ConnectorSecretError {
    fn from(error: SecretBackendError) -> Self {
        match error {
            SecretBackendError::Unavailable => Self::Unavailable,
            SecretBackendError::Denied => Self::Denied,
        }
    }
}

/// Shared runtime context for connector handlers and the flow executor.
#[derive(Clone, Default)]
pub(crate) struct ConnectorRuntime {
    secret_backend: Option<Arc<dyn SecretBackend + Send + Sync>>,
    #[cfg(test)]
    pub(crate) github_transport: Option<Arc<dyn GitHubTransport>>,
}

impl fmt::Debug for ConnectorRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConnectorRuntime")
            .field("injected_backend", &self.secret_backend.is_some())
            .finish_non_exhaustive()
    }
}

impl ConnectorRuntime {
    #[must_use]
    pub(crate) fn new(secret_backend: Option<Arc<dyn SecretBackend + Send + Sync>>) -> Self {
        Self {
            secret_backend,
            #[cfg(test)]
            github_transport: None,
        }
    }

    /// Runs one credential-store operation at a blocking boundary.
    ///
    /// Without an injected backend the operating-system store is opened per
    /// operation, mirroring the CLI's per-command access pattern.
    async fn with_backend<T>(
        &self,
        operation: impl FnOnce(&dyn SecretBackend) -> Result<T, SecretBackendError> + Send + 'static,
    ) -> Result<T, ConnectorSecretError>
    where
        T: Send + 'static,
    {
        let injected = self.secret_backend.clone();
        tokio::task::spawn_blocking(move || {
            if let Some(backend) = injected {
                operation(backend.as_ref())
            } else {
                let backend = NativeSecretBackend::new()?;
                operation(&backend)
            }
        })
        .await
        .map_err(|_| ConnectorSecretError::Unavailable)?
        .map_err(ConnectorSecretError::from)
    }

    pub(crate) async fn credential_present(
        &self,
        connector_id: &str,
    ) -> Result<bool, ConnectorSecretError> {
        Ok(self.load_credential(connector_id).await?.is_some())
    }

    /// Loads a stored connector credential, when one exists.
    ///
    /// The returned value must never be logged or embedded in any result.
    pub(crate) async fn load_credential(
        &self,
        connector_id: &str,
    ) -> Result<Option<String>, ConnectorSecretError> {
        let locator = connector_locator(connector_id)?;
        let credential = self
            .with_backend(move |backend| backend.get(&locator))
            .await?;
        match credential {
            Some(credential) if valid_connector_secret(credential.expose_secret()) => {
                Ok(Some(credential.expose_secret().to_owned()))
            }
            Some(_) => Err(ConnectorSecretError::InvalidSecret),
            None => Ok(None),
        }
    }

    pub(crate) async fn set_credential(
        &self,
        connector_id: &str,
        secret: String,
    ) -> Result<(), ConnectorSecretError> {
        if !valid_connector_secret(&secret) {
            return Err(ConnectorSecretError::InvalidSecret);
        }
        let locator = connector_locator(connector_id)?;
        self.with_backend(move |backend| backend.set(&locator, &CallerCredential::new(secret)))
            .await
    }

    /// Removes a stored connector credential; a missing entry is not an error.
    pub(crate) async fn clear_credential(
        &self,
        connector_id: &str,
    ) -> Result<(), ConnectorSecretError> {
        let locator = connector_locator(connector_id)?;
        self.with_backend(move |backend| backend.delete(&locator).map(drop))
            .await
    }

    /// Builds the GitHub connector against a validated HTTPS API base.
    ///
    /// # Errors
    ///
    /// Returns a sanitized error when the base URL or token is invalid, or the
    /// hardened HTTPS client cannot be initialized.
    pub(crate) fn github(
        &self,
        base_url: Option<&str>,
        token: String,
    ) -> Result<GitHubActions<Arc<dyn GitHubTransport>>, ConnectorSecretError> {
        let transport = self.github_transport_for(token)?;
        GitHubActions::with_base_str(base_url.unwrap_or(GITHUB_DEFAULT_API_BASE), transport)
            .map_err(|_| ConnectorSecretError::InvalidSecret)
    }

    #[cfg_attr(not(test), allow(clippy::unused_self))] // The test seam lives on `self`.
    fn github_transport_for(
        &self,
        token: String,
    ) -> Result<Arc<dyn GitHubTransport>, ConnectorSecretError> {
        #[cfg(test)]
        if let Some(transport) = &self.github_transport {
            drop(token);
            return Ok(Arc::clone(transport));
        }
        production_github_transport(token)
    }
}

fn production_github_transport(
    token: String,
) -> Result<Arc<dyn GitHubTransport>, ConnectorSecretError> {
    // The connector's own transport is the hardened production client: rustls
    // with native verification, bounded timeouts, and no implicit redirects
    // (the connector validates redirect targets itself).
    let transport = ReqwestGitHubTransport::new(Some(token))
        .map_err(|_| ConnectorSecretError::InvalidSecret)?;
    Ok(Arc::new(transport))
}

fn connector_locator(connector_id: &str) -> Result<SecretLocator, ConnectorSecretError> {
    SecretLocator::for_connector(connector_id).map_err(|_| ConnectorSecretError::InvalidSecret)
}

fn valid_connector_secret(secret: &str) -> bool {
    !secret.is_empty()
        && secret.len() <= MAX_CONNECTOR_SECRET_BYTES
        && !secret.chars().any(char::is_control)
}
