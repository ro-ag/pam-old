//! The daemon's built-in connector registry and its credential seam.
//!
//! Connector configuration lives in the durable store; credentials live only
//! in the operating system's native credential store under a connector-specific
//! locator domain. Credential values never appear in logs, results, or errors.

use std::{fmt, sync::Arc};

use pam_connectors::confluence::{Confluence, ConfluenceTransport, ReqwestConfluenceTransport};
use pam_connectors::github::{GitHubActions, GitHubTransport, ReqwestGitHubTransport};
use pam_connectors::jenkins::{Jenkins, JenkinsTransport, ReqwestJenkinsTransport};
use pam_connectors::jira::{Jira, JiraTransport, ReqwestJiraTransport};
use pam_connectors::sonarqube::{ReqwestSonarTransport, SonarQube, SonarTransport};
use pam_core::CallerCredential;
use pam_platform::{NativeSecretBackend, SecretBackend, SecretBackendError, SecretLocator};

pub(crate) const GITHUB_ACTIONS: &str = "github-actions";
pub(crate) const JENKINS: &str = "jenkins";
pub(crate) const SONARQUBE: &str = "sonarqube";
pub(crate) const JIRA: &str = "jira";
pub(crate) const CONFLUENCE: &str = "confluence";
pub(crate) const GITHUB_DEFAULT_API_BASE: &str = "https://api.github.com/";
pub(crate) const MAX_CONNECTOR_SECRET_BYTES: usize = 4096;

/// The flow-step capability names executable through the GitHub connector.
pub(crate) const GITHUB_DISCOVER_CAPABILITY: &str = "runs.discover-failed";
pub(crate) const GITHUB_COLLECT_LOGS_CAPABILITY: &str = "runs.collect-logs";

/// The flow-step capability names executable through the Jenkins connector.
pub(crate) const JENKINS_DISCOVER_JOBS_CAPABILITY: &str = "jobs.discover";
pub(crate) const JENKINS_DISCOVER_BUILDS_CAPABILITY: &str = "builds.discover";
pub(crate) const JENKINS_COLLECT_LOG_CAPABILITY: &str = "builds.collect-log";

/// The flow-step capability names executable through the `SonarQube` connector.
pub(crate) const SONARQUBE_GATE_CAPABILITY: &str = "gate.inspect";
pub(crate) const SONARQUBE_ISSUES_CAPABILITY: &str = "issues.discover";

/// The flow-step capability names executable through the Jira connector.
pub(crate) const JIRA_DISCOVER_ISSUES_CAPABILITY: &str = "issues.discover";
pub(crate) const JIRA_COLLECT_ISSUE_CAPABILITY: &str = "issues.collect";

/// The flow-step capability names executable through the Confluence connector.
pub(crate) const CONFLUENCE_DISCOVER_PAGES_CAPABILITY: &str = "pages.discover";
pub(crate) const CONFLUENCE_COLLECT_PAGE_CAPABILITY: &str = "pages.collect";

#[must_use]
pub(crate) fn built_in_connector_ids() -> [&'static str; 5] {
    [GITHUB_ACTIONS, JENKINS, SONARQUBE, JIRA, CONFLUENCE]
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
    #[cfg(test)]
    pub(crate) jenkins_transport: Option<Arc<dyn JenkinsTransport>>,
    #[cfg(test)]
    pub(crate) sonarqube_transport: Option<Arc<dyn SonarTransport>>,
    #[cfg(test)]
    pub(crate) jira_transport: Option<Arc<dyn JiraTransport>>,
    #[cfg(test)]
    pub(crate) confluence_transport: Option<Arc<dyn ConfluenceTransport>>,
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
            #[cfg(test)]
            jenkins_transport: None,
            #[cfg(test)]
            sonarqube_transport: None,
            #[cfg(test)]
            jira_transport: None,
            #[cfg(test)]
            confluence_transport: None,
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

    /// Builds the Jenkins connector against a validated HTTPS API base.
    ///
    /// # Errors
    ///
    /// Returns a sanitized error when the base URL or `user:token` secret is
    /// invalid, or the hardened HTTPS client cannot be initialized.
    pub(crate) fn jenkins(
        &self,
        base_url: &str,
        secret: String,
    ) -> Result<Jenkins<Arc<dyn JenkinsTransport>>, ConnectorSecretError> {
        let transport = self.jenkins_transport_for(secret)?;
        Jenkins::with_base_str(base_url, transport).map_err(|_| ConnectorSecretError::InvalidSecret)
    }

    #[cfg_attr(not(test), allow(clippy::unused_self))] // The test seam lives on `self`.
    fn jenkins_transport_for(
        &self,
        secret: String,
    ) -> Result<Arc<dyn JenkinsTransport>, ConnectorSecretError> {
        #[cfg(test)]
        if let Some(transport) = &self.jenkins_transport {
            drop(secret);
            return Ok(Arc::clone(transport));
        }
        production_jenkins_transport(secret)
    }

    /// Builds the `SonarQube` connector against a validated HTTPS API base.
    ///
    /// # Errors
    ///
    /// Returns a sanitized error when the base URL or token is invalid, or the
    /// hardened HTTPS client cannot be initialized.
    pub(crate) fn sonarqube(
        &self,
        base_url: &str,
        token: String,
    ) -> Result<SonarQube<Arc<dyn SonarTransport>>, ConnectorSecretError> {
        let transport = self.sonarqube_transport_for(token)?;
        SonarQube::with_base_str(base_url, transport)
            .map_err(|_| ConnectorSecretError::InvalidSecret)
    }

    #[cfg_attr(not(test), allow(clippy::unused_self))] // The test seam lives on `self`.
    fn sonarqube_transport_for(
        &self,
        token: String,
    ) -> Result<Arc<dyn SonarTransport>, ConnectorSecretError> {
        #[cfg(test)]
        if let Some(transport) = &self.sonarqube_transport {
            drop(token);
            return Ok(Arc::clone(transport));
        }
        production_sonarqube_transport(token)
    }

    /// Builds the Jira connector against a validated HTTPS API base.
    ///
    /// # Errors
    ///
    /// Returns a sanitized error when the base URL or token is invalid, or the
    /// hardened HTTPS client cannot be initialized.
    pub(crate) fn jira(
        &self,
        base_url: &str,
        token: String,
    ) -> Result<Jira<Arc<dyn JiraTransport>>, ConnectorSecretError> {
        let transport = self.jira_transport_for(token)?;
        Jira::with_base_str(base_url, transport).map_err(|_| ConnectorSecretError::InvalidSecret)
    }

    #[cfg_attr(not(test), allow(clippy::unused_self))] // The test seam lives on `self`.
    fn jira_transport_for(
        &self,
        token: String,
    ) -> Result<Arc<dyn JiraTransport>, ConnectorSecretError> {
        #[cfg(test)]
        if let Some(transport) = &self.jira_transport {
            drop(token);
            return Ok(Arc::clone(transport));
        }
        production_jira_transport(token)
    }

    /// Builds the Confluence connector against a validated HTTPS API base.
    ///
    /// # Errors
    ///
    /// Returns a sanitized error when the base URL or `email:api-token` secret
    /// is invalid, or the hardened HTTPS client cannot be initialized.
    pub(crate) fn confluence(
        &self,
        base_url: &str,
        secret: String,
    ) -> Result<Confluence<Arc<dyn ConfluenceTransport>>, ConnectorSecretError> {
        let transport = self.confluence_transport_for(secret)?;
        Confluence::with_base_str(base_url, transport)
            .map_err(|_| ConnectorSecretError::InvalidSecret)
    }

    #[cfg_attr(not(test), allow(clippy::unused_self))] // The test seam lives on `self`.
    fn confluence_transport_for(
        &self,
        secret: String,
    ) -> Result<Arc<dyn ConfluenceTransport>, ConnectorSecretError> {
        #[cfg(test)]
        if let Some(transport) = &self.confluence_transport {
            drop(secret);
            return Ok(Arc::clone(transport));
        }
        production_confluence_transport(secret)
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

fn production_jenkins_transport(
    secret: String,
) -> Result<Arc<dyn JenkinsTransport>, ConnectorSecretError> {
    // The stored connector secret is one combined `user:api-token` value; the
    // transport splits it and authenticates with HTTP Basic over rustls.
    let transport = ReqwestJenkinsTransport::new(Some(secret))
        .map_err(|_| ConnectorSecretError::InvalidSecret)?;
    Ok(Arc::new(transport))
}

fn production_sonarqube_transport(
    token: String,
) -> Result<Arc<dyn SonarTransport>, ConnectorSecretError> {
    // The stored connector secret is one SonarQube user token; the transport
    // sends it as the HTTP Basic username with an empty password over rustls.
    let transport =
        ReqwestSonarTransport::new(Some(token)).map_err(|_| ConnectorSecretError::InvalidSecret)?;
    Ok(Arc::new(transport))
}

fn production_jira_transport(
    token: String,
) -> Result<Arc<dyn JiraTransport>, ConnectorSecretError> {
    // The stored connector secret is one Jira Data Center personal access
    // token; the transport sends it as an HTTP Bearer credential over rustls.
    let transport =
        ReqwestJiraTransport::new(Some(token)).map_err(|_| ConnectorSecretError::InvalidSecret)?;
    Ok(Arc::new(transport))
}

fn production_confluence_transport(
    secret: String,
) -> Result<Arc<dyn ConfluenceTransport>, ConnectorSecretError> {
    // The stored connector secret is one combined `email:api-token` value; the
    // transport splits it and authenticates with HTTP Basic over rustls.
    let transport = ReqwestConfluenceTransport::new(Some(secret))
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
