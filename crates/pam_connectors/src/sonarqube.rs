//! Typed `SonarQube` operations.

use std::{collections::BTreeMap, error::Error, fmt, sync::Arc, time::Duration};

use reqwest::{StatusCode, header};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use url::Url;

use crate::{
    BoundedSummary, CapabilityName, Connector, ConnectorDescriptor, ConnectorFailure,
    ConnectorFuture, ConnectorOutput, FailureKind, FailureMessage, InvocationContext, Operation,
    OperationCoordinates, OperationEffect, ResourceName, Truth, github::classify_transport_failure,
};

pub const MAX_DISCOVERED_ISSUES: usize = 100;
pub const MAX_JSON_BYTES: usize = 1024 * 1024;
pub const MAX_PROJECT_KEY_BYTES: usize = 400;

const MAX_REMOTE_TEXT_BYTES: usize = 2048;
const MAX_SECRET_BYTES: usize = 4096;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// A validated `SonarQube` project key.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct ProjectKey {
    key: String,
}

impl ProjectKey {
    /// Parses a bounded `SonarQube` project key.
    ///
    /// # Errors
    ///
    /// Returns an error unless the key is one to 400 bytes of ASCII
    /// alphanumerics, `-`, `_`, `.`, or `:`.
    pub fn parse(value: impl AsRef<str>) -> Result<Self, InvalidProjectKey> {
        let key = value.as_ref();
        if key.is_empty()
            || key.len() > MAX_PROJECT_KEY_BYTES
            || !key.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':')
            })
        {
            return Err(InvalidProjectKey);
        }
        Ok(Self {
            key: key.to_owned(),
        })
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.key
    }

    #[must_use]
    pub fn artifact_slug(&self) -> String {
        self.key.replace(':', "-")
    }

    fn resource(&self) -> ResourceName {
        ResourceName::parse(format!("sonarqube:{}", self.key))
            .expect("validated SonarQube keys fit the policy resource bound")
    }

    fn issues_resource(&self) -> ResourceName {
        ResourceName::parse(format!("sonarqube:{}/issues", self.key))
            .expect("validated SonarQube issue coordinates fit the policy resource bound")
    }
}

impl fmt::Debug for ProjectKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.key)
    }
}

impl<'de> Deserialize<'de> for ProjectKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

impl Serialize for ProjectKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.key)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidProjectKey;

impl fmt::Display for InvalidProjectKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .write_str("SonarQube project key must be one to 400 bounded safe ASCII characters")
    }
}

impl Error for InvalidProjectKey {}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FetchQualityGateRequest {
    project: ProjectKey,
}

impl FetchQualityGateRequest {
    #[must_use]
    pub const fn new(project: ProjectKey) -> Self {
        Self { project }
    }

    #[must_use]
    pub const fn project(&self) -> &ProjectKey {
        &self.project
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DiscoverIssuesRequest {
    project: ProjectKey,
    limit: usize,
}

impl DiscoverIssuesRequest {
    /// # Errors
    ///
    /// Returns an error unless the limit is between one and 100.
    pub fn new(project: ProjectKey, limit: usize) -> Result<Self, InvalidReadBound> {
        if !(1..=MAX_DISCOVERED_ISSUES).contains(&limit) {
            return Err(InvalidReadBound);
        }
        Ok(Self { project, limit })
    }

    #[must_use]
    pub const fn project(&self) -> &ProjectKey {
        &self.project
    }

    #[must_use]
    pub const fn limit(&self) -> usize {
        self.limit
    }

    fn validate(&self) -> Result<(), ConnectorFailure> {
        if (1..=MAX_DISCOVERED_ISSUES).contains(&self.limit) {
            Ok(())
        } else {
            Err(invalid_request(
                "SonarQube issue discovery limit is invalid",
            ))
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidReadBound;

impl fmt::Display for InvalidReadBound {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SonarQube read limit exceeds the connector SDK bound")
    }
}

impl Error for InvalidReadBound {}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GateCondition {
    metric_key: String,
    status: String,
    comparator: Option<String>,
    error_threshold: Option<String>,
    actual_value: Option<String>,
}

impl GateCondition {
    #[must_use]
    pub fn metric_key(&self) -> &str {
        &self.metric_key
    }

    #[must_use]
    pub fn status(&self) -> &str {
        &self.status
    }

    #[must_use]
    pub fn comparator(&self) -> Option<&str> {
        self.comparator.as_deref()
    }

    #[must_use]
    pub fn error_threshold(&self) -> Option<&str> {
        self.error_threshold.as_deref()
    }

    #[must_use]
    pub fn actual_value(&self) -> Option<&str> {
        self.actual_value.as_deref()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FetchQualityGateResponse {
    project: String,
    status: String,
    failed_conditions: Vec<GateCondition>,
}

impl FetchQualityGateResponse {
    #[must_use]
    pub fn project(&self) -> &str {
        &self.project
    }

    #[must_use]
    pub fn status(&self) -> &str {
        &self.status
    }

    #[must_use]
    pub fn failed_conditions(&self) -> &[GateCondition] {
        &self.failed_conditions
    }

    #[must_use]
    pub fn is_passing(&self) -> bool {
        self.status == "OK"
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SonarIssue {
    key: String,
    rule: String,
    severity: String,
    component: String,
    line: Option<u64>,
    message: String,
    #[serde(rename = "type")]
    issue_type: String,
}

impl SonarIssue {
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    #[must_use]
    pub fn rule(&self) -> &str {
        &self.rule
    }

    #[must_use]
    pub fn severity(&self) -> &str {
        &self.severity
    }

    #[must_use]
    pub fn component(&self) -> &str {
        &self.component
    }

    #[must_use]
    pub const fn line(&self) -> Option<u64> {
        self.line
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    #[must_use]
    pub fn issue_type(&self) -> &str {
        &self.issue_type
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DiscoverIssuesResponse {
    project: String,
    total: u64,
    issues: Vec<SonarIssue>,
}

impl DiscoverIssuesResponse {
    #[must_use]
    pub fn project(&self) -> &str {
        &self.project
    }

    #[must_use]
    pub const fn total(&self) -> u64 {
        self.total
    }

    #[must_use]
    pub fn issues(&self) -> &[SonarIssue] {
        &self.issues
    }
}

pub struct FetchQualityGate;

impl Operation for FetchQualityGate {
    type Request = FetchQualityGateRequest;
    type Response = FetchQualityGateResponse;

    const EFFECT: OperationEffect = OperationEffect::ReadOnly;

    fn coordinates(request: &Self::Request) -> OperationCoordinates {
        OperationCoordinates::new(capability(), request.project.resource())
    }
}

pub struct DiscoverIssues;

impl Operation for DiscoverIssues {
    type Request = DiscoverIssuesRequest;
    type Response = DiscoverIssuesResponse;

    const EFFECT: OperationEffect = OperationEffect::ReadOnly;

    fn coordinates(request: &Self::Request) -> OperationCoordinates {
        OperationCoordinates::new(capability(), request.project.issues_resource())
    }
}

/// Minimal authenticated read-only probe used by connector self-tests.
///
/// It verifies base-URL reachability, TLS, and the stored token by asking the
/// server to validate the current authentication; no project data is read and
/// no remote identity details are returned.
pub struct VerifyCredentials;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct VerifyCredentialsRequest {}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct VerifyCredentialsResponse {}

impl Operation for VerifyCredentials {
    type Request = VerifyCredentialsRequest;
    type Response = VerifyCredentialsResponse;

    const EFFECT: OperationEffect = OperationEffect::ReadOnly;

    fn coordinates(_request: &Self::Request) -> OperationCoordinates {
        OperationCoordinates::new(
            CapabilityName::parse("connection.verify")
                .expect("static SonarQube capability is valid"),
            ResourceName::parse("sonarqube:api").expect("static SonarQube resource is valid"),
        )
    }
}

/// `SonarQube` connector over an injected bounded HTTP transport.
pub struct SonarQube<T> {
    api_base: Url,
    transport: T,
}

impl<T> SonarQube<T> {
    /// # Errors
    ///
    /// Returns an error unless the API base is a credential-free HTTPS hierarchy.
    pub fn new(api_base: Url, transport: T) -> Result<Self, InvalidSonarConfiguration> {
        validate_https_url(&api_base)?;
        Ok(Self {
            api_base: normalized_base(api_base),
            transport,
        })
    }

    /// Parses and validates a textual API base for callers without a URL type.
    ///
    /// # Errors
    ///
    /// Returns an error unless the API base parses as a credential-free HTTPS
    /// hierarchy.
    pub fn with_base_str(api_base: &str, transport: T) -> Result<Self, InvalidSonarConfiguration> {
        let api_base = Url::parse(api_base).map_err(|_| InvalidSonarConfiguration)?;
        Self::new(api_base, transport)
    }

    #[must_use]
    pub const fn transport(&self) -> &T {
        &self.transport
    }

    fn api_request(
        &self,
        path: &str,
        response_limit: usize,
    ) -> Result<TransportRequest, ConnectorFailure> {
        let url = self
            .api_base
            .join(path)
            .map_err(|_| invalid_request("SonarQube API request path is invalid"))?;
        Ok(TransportRequest {
            url,
            authenticated: true,
            response_limit,
        })
    }
}

impl<T: SonarTransport> Connector<FetchQualityGate> for SonarQube<T> {
    fn descriptor(&self) -> ConnectorDescriptor {
        descriptor()
    }

    fn execute(
        &self,
        request: FetchQualityGateRequest,
        context: InvocationContext,
    ) -> ConnectorFuture<'_, crate::ConnectorResult<FetchQualityGateResponse>> {
        Box::pin(async move {
            context.preflight(OperationEffect::ReadOnly)?;
            let path = format!(
                "api/qualitygates/project_status?projectKey={}",
                request.project.as_str()
            );
            let response = self
                .transport
                .get(self.api_request(&path, MAX_JSON_BYTES)?, &context)
                .await?;
            require_status(&response, StatusCode::OK)?;
            let envelope: ProjectStatusEnvelope = parse_json(&response.body)?;
            let status = envelope.project_status.status;
            if !valid_remote_text(&status) {
                return Err(remote_failure("SonarQube quality gate status was invalid"));
            }
            let failed_conditions = envelope
                .project_status
                .conditions
                .into_iter()
                .filter(|condition| condition.status != "OK")
                .collect::<Vec<_>>();
            for condition in &failed_conditions {
                validate_gate_condition(condition)?;
            }
            let project = request.project.as_str().to_owned();
            let count = failed_conditions.len();
            ConnectorOutput::new(
                FetchQualityGateResponse {
                    project: project.clone(),
                    status: status.clone(),
                    failed_conditions,
                },
                summary(format!(
                    "SonarQube quality gate for {project} is {status} with {count} failed \
                     condition(s)"
                ))?,
                Truth::Complete,
                Vec::new(),
                Vec::new(),
            )
            .map_err(|_| remote_failure("SonarQube quality gate output exceeded SDK bounds"))
        })
    }
}

impl<T: SonarTransport> Connector<DiscoverIssues> for SonarQube<T> {
    fn descriptor(&self) -> ConnectorDescriptor {
        descriptor()
    }

    fn execute(
        &self,
        request: DiscoverIssuesRequest,
        context: InvocationContext,
    ) -> ConnectorFuture<'_, crate::ConnectorResult<DiscoverIssuesResponse>> {
        Box::pin(async move {
            context.preflight(OperationEffect::ReadOnly)?;
            request.validate()?;
            let path = format!(
                "api/issues/search?componentKeys={}&resolved=false&ps={}",
                request.project.as_str(),
                request.limit
            );
            let response = self
                .transport
                .get(self.api_request(&path, MAX_JSON_BYTES)?, &context)
                .await?;
            require_status(&response, StatusCode::OK)?;
            let mut envelope: IssuesEnvelope = parse_json(&response.body)?;
            if envelope.issues.len() > request.limit {
                return Err(remote_failure(
                    "SonarQube returned more issues than requested",
                ));
            }
            for issue in &envelope.issues {
                validate_discovered_issue(issue)?;
            }
            envelope
                .issues
                .sort_by(|left, right| left.key.cmp(&right.key));
            let count = envelope.issues.len();
            let retained = u64::try_from(count).expect("bounded issue count fits u64");
            let project = request.project.as_str().to_owned();
            let truth = if envelope.total > retained {
                Truth::Partial {
                    reason: summary(format!(
                        "retained {retained} of {} unresolved issues",
                        envelope.total
                    ))?,
                }
            } else {
                Truth::Complete
            };
            ConnectorOutput::new(
                DiscoverIssuesResponse {
                    project: project.clone(),
                    total: envelope.total,
                    issues: envelope.issues,
                },
                summary(format!(
                    "found {count} unresolved SonarQube issues for {project}"
                ))?,
                truth,
                Vec::new(),
                Vec::new(),
            )
            .map_err(|_| remote_failure("SonarQube issue discovery output exceeded SDK bounds"))
        })
    }
}

impl<T: SonarTransport> Connector<VerifyCredentials> for SonarQube<T> {
    fn descriptor(&self) -> ConnectorDescriptor {
        descriptor()
    }

    fn execute(
        &self,
        _request: VerifyCredentialsRequest,
        context: InvocationContext,
    ) -> ConnectorFuture<'_, crate::ConnectorResult<VerifyCredentialsResponse>> {
        Box::pin(async move {
            context.preflight(OperationEffect::ReadOnly)?;
            let response = self
                .transport
                .get(
                    self.api_request("api/authentication/validate", MAX_JSON_BYTES)?,
                    &context,
                )
                .await?;
            require_status(&response, StatusCode::OK)?;
            let validation: ValidationEnvelope = parse_json(&response.body)?;
            if !validation.valid {
                return Err(ConnectorFailure::authentication(safe_message(
                    "SonarQube rejected the stored token as not valid",
                )));
            }
            ConnectorOutput::new(
                VerifyCredentialsResponse {},
                summary("SonarQube token and API base verified".to_owned())?,
                Truth::Complete,
                Vec::new(),
                Vec::new(),
            )
            .map_err(|_| remote_failure("SonarQube verification output exceeded SDK bounds"))
        })
    }
}

#[derive(Deserialize)]
struct ProjectStatusEnvelope {
    #[serde(rename = "projectStatus")]
    project_status: ProjectStatusBody,
}

#[derive(Deserialize)]
struct ProjectStatusBody {
    status: String,
    #[serde(default)]
    conditions: Vec<GateCondition>,
}

#[derive(Deserialize)]
struct IssuesEnvelope {
    total: u64,
    issues: Vec<SonarIssue>,
}

#[derive(Deserialize)]
struct ValidationEnvelope {
    valid: bool,
}

/// Transport request kept crate-visible for deterministic connector tests.
pub struct TransportRequest {
    url: Url,
    authenticated: bool,
    response_limit: usize,
}

impl TransportRequest {
    #[must_use]
    pub fn url(&self) -> &Url {
        &self.url
    }

    #[must_use]
    pub const fn authenticated(&self) -> bool {
        self.authenticated
    }

    #[must_use]
    pub const fn response_limit(&self) -> usize {
        self.response_limit
    }
}

/// Bounded HTTP response kept crate-visible for deterministic connector tests.
pub struct TransportResponse {
    status: u16,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

impl TransportResponse {
    pub fn new(
        status: u16,
        headers: impl IntoIterator<Item = (String, String)>,
        body: Vec<u8>,
    ) -> Self {
        Self {
            status,
            headers: headers
                .into_iter()
                .map(|(name, value)| (name.to_ascii_lowercase(), value))
                .collect(),
            body,
        }
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }
}

pub trait SonarTransport: Send + Sync {
    fn get<'a>(
        &'a self,
        request: TransportRequest,
        context: &'a InvocationContext,
    ) -> ConnectorFuture<'a, Result<TransportResponse, ConnectorFailure>>;
}

impl<T: SonarTransport + ?Sized> SonarTransport for Arc<T> {
    fn get<'a>(
        &'a self,
        request: TransportRequest,
        context: &'a InvocationContext,
    ) -> ConnectorFuture<'a, Result<TransportResponse, ConnectorFailure>> {
        (**self).get(request, context)
    }
}

/// Production transport with native rustls verification, system proxy support, and no redirects.
pub struct ReqwestSonarTransport {
    client: reqwest::Client,
    token: Option<String>,
}

impl ReqwestSonarTransport {
    /// Accepts an optional `SonarQube` user token sent as the HTTP Basic username
    /// with an empty password.
    ///
    /// # Errors
    ///
    /// Returns a sanitized error when the token or secure HTTP client is invalid.
    pub fn new(token: Option<String>) -> Result<Self, InvalidSonarConfiguration> {
        if let Some(token) = &token
            && !valid_token(token)
        {
            return Err(InvalidSonarConfiguration);
        }
        let client = reqwest::Client::builder()
            .use_rustls_tls()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| InvalidSonarConfiguration)?;
        Ok(Self { client, token })
    }
}

fn valid_token(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= MAX_SECRET_BYTES
        && !token.contains(':')
        && !token.chars().any(char::is_control)
}

impl fmt::Debug for ReqwestSonarTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReqwestSonarTransport")
            .field("authenticated", &self.token.is_some())
            .finish_non_exhaustive()
    }
}

impl SonarTransport for ReqwestSonarTransport {
    fn get<'a>(
        &'a self,
        request: TransportRequest,
        context: &'a InvocationContext,
    ) -> ConnectorFuture<'a, Result<TransportResponse, ConnectorFailure>> {
        Box::pin(async move {
            context.preflight(OperationEffect::ReadOnly)?;
            let remaining = context.remaining().ok_or_else(ConnectorFailure::timeout)?;
            let mut builder = self
                .client
                .get(request.url)
                .header(header::ACCEPT, "application/json")
                .header(header::USER_AGENT, "pam/0.1.0")
                .timeout(remaining.min(REQUEST_TIMEOUT));
            if request.authenticated
                && let Some(token) = &self.token
            {
                builder = builder.basic_auth(token, Some(""));
            }
            let mut response = builder
                .send()
                .await
                .map_err(|error| map_reqwest_failure(&error))?;
            let status = response.status().as_u16();
            let mut headers = BTreeMap::new();
            if let Some(value) = response.headers().get(header::RETRY_AFTER)
                && let Ok(value) = value.to_str()
                && value.len() <= MAX_REMOTE_TEXT_BYTES
                && !value.chars().any(char::is_control)
            {
                headers.insert(header::RETRY_AFTER.as_str().to_owned(), value.to_owned());
            }
            if response
                .content_length()
                .is_some_and(|length| length > request.response_limit as u64)
            {
                return Err(ConnectorFailure::response_too_large(request.response_limit));
            }
            let mut body = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|error| map_reqwest_failure(&error))?
            {
                context.preflight(OperationEffect::ReadOnly)?;
                if body.len().saturating_add(chunk.len()) > request.response_limit {
                    return Err(ConnectorFailure::response_too_large(request.response_limit));
                }
                body.extend_from_slice(&chunk);
            }
            Ok(TransportResponse {
                status,
                headers,
                body,
            })
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidSonarConfiguration;

impl fmt::Display for InvalidSonarConfiguration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SonarQube connector configuration is invalid")
    }
}

impl Error for InvalidSonarConfiguration {}

fn descriptor() -> ConnectorDescriptor {
    ConnectorDescriptor::new("sonarqube", "web-api-1")
        .expect("static SonarQube connector descriptor is valid")
}

fn capability() -> CapabilityName {
    CapabilityName::parse("projects.inspect").expect("static SonarQube capability is valid")
}

fn summary(value: String) -> Result<BoundedSummary, ConnectorFailure> {
    BoundedSummary::new(value)
        .map_err(|_| remote_failure("SonarQube connector summary exceeded SDK bounds"))
}

fn parse_json<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, ConnectorFailure> {
    serde_json::from_slice(bytes)
        .map_err(|_| remote_failure("SonarQube returned malformed JSON metadata"))
}

fn require_status(
    response: &TransportResponse,
    expected: StatusCode,
) -> Result<(), ConnectorFailure> {
    if response.status == expected.as_u16() {
        return Ok(());
    }
    let message = safe_message("SonarQube API request failed");
    match response.status {
        401 => Err(ConnectorFailure::authentication(message)),
        403 => Err(ConnectorFailure::forbidden(message)),
        404 => Err(ConnectorFailure::not_found(message)),
        429 => Err(ConnectorFailure::rate_limit(message, retry_delay(response))),
        500..=599 => Err(ConnectorFailure::remote(message, true)),
        _ => Err(ConnectorFailure::remote(message, false)),
    }
}

fn retry_delay(response: &TransportResponse) -> Option<Duration> {
    response
        .header("retry-after")
        .and_then(|value| value.parse::<u64>().ok())
        .map(|seconds| Duration::from_secs(seconds.min(24 * 60 * 60)))
}

fn map_reqwest_failure(error: &reqwest::Error) -> ConnectorFailure {
    match classify_transport_failure(error, error.is_timeout(), error.is_connect()) {
        FailureKind::Timeout => ConnectorFailure::timeout(),
        FailureKind::Certificate => {
            ConnectorFailure::certificate(safe_message("SonarQube certificate verification failed"))
        }
        FailureKind::Network => {
            ConnectorFailure::network(safe_message("SonarQube network connection failed"))
        }
        FailureKind::Remote => {
            ConnectorFailure::remote(safe_message("SonarQube HTTP request failed"), true)
        }
        _ => unreachable!("transport classifier returned a non-transport failure"),
    }
}

fn validate_gate_condition(condition: &GateCondition) -> Result<(), ConnectorFailure> {
    let optional_valid = |value: Option<&str>| value.is_none_or(valid_remote_text);
    if !valid_remote_text(&condition.metric_key)
        || !valid_remote_text(&condition.status)
        || !optional_valid(condition.comparator.as_deref())
        || !optional_valid(condition.error_threshold.as_deref())
        || !optional_valid(condition.actual_value.as_deref())
    {
        return Err(remote_failure(
            "SonarQube gate condition metadata was invalid",
        ));
    }
    Ok(())
}

fn validate_discovered_issue(issue: &SonarIssue) -> Result<(), ConnectorFailure> {
    if !valid_remote_text(&issue.key)
        || !valid_remote_text(&issue.rule)
        || !valid_remote_text(&issue.severity)
        || !valid_remote_text(&issue.component)
        || !valid_remote_text(&issue.message)
        || !valid_remote_text(&issue.issue_type)
    {
        return Err(remote_failure("SonarQube issue metadata was invalid"));
    }
    Ok(())
}

fn valid_remote_text(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_REMOTE_TEXT_BYTES
        && !value.chars().any(char::is_control)
}

fn validate_https_url(url: &Url) -> Result<(), InvalidSonarConfiguration> {
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.cannot_be_a_base()
    {
        return Err(InvalidSonarConfiguration);
    }
    Ok(())
}

fn normalized_base(mut url: Url) -> Url {
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    url
}

fn invalid_request(message: &str) -> ConnectorFailure {
    ConnectorFailure::invalid_request(safe_message(message))
}

fn remote_failure(message: &str) -> ConnectorFailure {
    ConnectorFailure::remote(safe_message(message), false)
}

fn safe_message(value: &str) -> FailureMessage {
    FailureMessage::new(value).expect("static SonarQube failure message is valid")
}
