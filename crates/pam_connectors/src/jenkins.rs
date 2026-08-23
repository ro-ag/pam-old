//! Typed Jenkins operations.

use std::{collections::BTreeMap, error::Error, fmt, num::NonZeroU64, sync::Arc, time::Duration};

use reqwest::{StatusCode, header};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use url::Url;

use crate::{
    BoundedSummary, CapabilityName, Connector, ConnectorDescriptor, ConnectorFailure,
    ConnectorFuture, ConnectorOutput, ExactArtifact, FailureKind, FailureMessage,
    InvocationContext, Operation, OperationCoordinates, OperationEffect, ResourceName, Truth,
    github::classify_transport_failure,
};

pub const MAX_DISCOVERED_JOBS: usize = 100;
pub const MAX_DISCOVERED_BUILDS: usize = 100;
pub const MAX_CONSOLE_LOG_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_JSON_BYTES: usize = 1024 * 1024;
pub const MAX_JOB_SEGMENTS: usize = 10;

const MAX_JOB_SEGMENT_BYTES: usize = 100;
const MAX_REMOTE_TEXT_BYTES: usize = 2048;
const MAX_SECRET_PART_BYTES: usize = 4096;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// A validated Jenkins job coordinate, optionally nested in folders.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct JobPath {
    segments: Vec<String>,
}

impl JobPath {
    /// Parses `job` or `folder/job` without accepting traversal or empty segments.
    ///
    /// # Errors
    ///
    /// Returns an error for anything other than one to ten bounded Jenkins path segments.
    pub fn parse(value: impl AsRef<str>) -> Result<Self, InvalidJobPath> {
        let segments = value
            .as_ref()
            .split('/')
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if segments.is_empty()
            || segments.len() > MAX_JOB_SEGMENTS
            || segments.iter().any(|segment| !valid_job_segment(segment))
        {
            return Err(InvalidJobPath);
        }
        Ok(Self { segments })
    }

    #[must_use]
    pub fn as_coordinate(&self) -> String {
        self.segments.join("/")
    }

    fn api_path(&self) -> String {
        self.segments
            .iter()
            .map(|segment| format!("job/{segment}"))
            .collect::<Vec<_>>()
            .join("/")
    }

    fn artifact_slug(&self) -> String {
        self.segments.join("-")
    }

    fn resource(&self) -> ResourceName {
        ResourceName::parse(format!("jenkins:{}", self.as_coordinate()))
            .expect("validated Jenkins coordinates fit the policy resource bound")
    }

    fn build_resource(&self, build: BuildNumber) -> ResourceName {
        ResourceName::parse(format!(
            "jenkins:{}/builds/{}",
            self.as_coordinate(),
            build.get()
        ))
        .expect("validated Jenkins build coordinates fit the policy resource bound")
    }
}

impl fmt::Debug for JobPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.as_coordinate())
    }
}

impl<'de> Deserialize<'de> for JobPath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

impl Serialize for JobPath {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.as_coordinate())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidJobPath;

impl fmt::Display for InvalidJobPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Jenkins job path must be one to ten bounded safe path segments")
    }
}

impl Error for InvalidJobPath {}

/// A nonzero Jenkins build number.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct BuildNumber(NonZeroU64);

impl BuildNumber {
    /// # Errors
    ///
    /// Returns an error when the build number is zero.
    pub fn new(value: u64) -> Result<Self, InvalidBuildNumber> {
        NonZeroU64::new(value).map(Self).ok_or(InvalidBuildNumber)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidBuildNumber;

impl fmt::Display for InvalidBuildNumber {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Jenkins build number must be nonzero")
    }
}

impl Error for InvalidBuildNumber {}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DiscoverJobsRequest {
    limit: usize,
}

impl DiscoverJobsRequest {
    /// # Errors
    ///
    /// Returns an error unless the limit is between one and 100.
    pub fn new(limit: usize) -> Result<Self, InvalidReadBound> {
        if !(1..=MAX_DISCOVERED_JOBS).contains(&limit) {
            return Err(InvalidReadBound);
        }
        Ok(Self { limit })
    }

    #[must_use]
    pub const fn limit(&self) -> usize {
        self.limit
    }

    fn validate(&self) -> Result<(), ConnectorFailure> {
        if (1..=MAX_DISCOVERED_JOBS).contains(&self.limit) {
            Ok(())
        } else {
            Err(invalid_request("Jenkins job discovery limit is invalid"))
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DiscoverBuildsRequest {
    job: JobPath,
    limit: usize,
}

impl DiscoverBuildsRequest {
    /// # Errors
    ///
    /// Returns an error unless the limit is between one and 100.
    pub fn new(job: JobPath, limit: usize) -> Result<Self, InvalidReadBound> {
        if !(1..=MAX_DISCOVERED_BUILDS).contains(&limit) {
            return Err(InvalidReadBound);
        }
        Ok(Self { job, limit })
    }

    #[must_use]
    pub const fn job(&self) -> &JobPath {
        &self.job
    }

    #[must_use]
    pub const fn limit(&self) -> usize {
        self.limit
    }

    fn validate(&self) -> Result<(), ConnectorFailure> {
        if (1..=MAX_DISCOVERED_BUILDS).contains(&self.limit) {
            Ok(())
        } else {
            Err(invalid_request("Jenkins build discovery limit is invalid"))
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CollectConsoleLogRequest {
    job: JobPath,
    build: BuildNumber,
    max_log_bytes: usize,
}

impl CollectConsoleLogRequest {
    /// Creates an explicit byte budget for one exact build's console log.
    ///
    /// # Errors
    ///
    /// Returns an error for a zero or SDK-exceeding byte limit.
    pub fn new(
        job: JobPath,
        build: BuildNumber,
        max_log_bytes: usize,
    ) -> Result<Self, InvalidReadBound> {
        if !(1..=MAX_CONSOLE_LOG_BYTES).contains(&max_log_bytes) {
            return Err(InvalidReadBound);
        }
        Ok(Self {
            job,
            build,
            max_log_bytes,
        })
    }

    #[must_use]
    pub const fn job(&self) -> &JobPath {
        &self.job
    }

    #[must_use]
    pub const fn build(&self) -> BuildNumber {
        self.build
    }

    fn validate(&self) -> Result<(), ConnectorFailure> {
        if (1..=MAX_CONSOLE_LOG_BYTES).contains(&self.max_log_bytes) {
            Ok(())
        } else {
            Err(invalid_request("Jenkins console log bounds are invalid"))
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidReadBound;

impl fmt::Display for InvalidReadBound {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Jenkins read limit exceeds the connector SDK bound")
    }
}

impl Error for InvalidReadBound {}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct JenkinsJob {
    name: String,
    url: String,
    color: Option<String>,
}

impl JenkinsJob {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    #[must_use]
    pub fn color(&self) -> Option<&str> {
        self.color.as_deref()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct JenkinsBuild {
    number: BuildNumber,
    result: Option<String>,
    timestamp: u64,
    duration: u64,
    url: String,
}

impl JenkinsBuild {
    #[must_use]
    pub const fn number(&self) -> BuildNumber {
        self.number
    }

    #[must_use]
    pub fn result(&self) -> Option<&str> {
        self.result.as_deref()
    }

    #[must_use]
    pub const fn timestamp(&self) -> u64 {
        self.timestamp
    }

    #[must_use]
    pub const fn duration(&self) -> u64 {
        self.duration
    }

    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DiscoverJobsResponse {
    jobs: Vec<JenkinsJob>,
}

impl DiscoverJobsResponse {
    #[must_use]
    pub fn jobs(&self) -> &[JenkinsJob] {
        &self.jobs
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DiscoverBuildsResponse {
    job: String,
    builds: Vec<JenkinsBuild>,
}

impl DiscoverBuildsResponse {
    #[must_use]
    pub fn job(&self) -> &str {
        &self.job
    }

    #[must_use]
    pub fn builds(&self) -> &[JenkinsBuild] {
        &self.builds
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CollectConsoleLogResponse {
    job: String,
    build: BuildNumber,
    artifact_name: String,
    byte_len: usize,
}

impl CollectConsoleLogResponse {
    #[must_use]
    pub fn job(&self) -> &str {
        &self.job
    }

    #[must_use]
    pub const fn build(&self) -> BuildNumber {
        self.build
    }

    #[must_use]
    pub fn artifact_name(&self) -> &str {
        &self.artifact_name
    }

    #[must_use]
    pub const fn byte_len(&self) -> usize {
        self.byte_len
    }
}

/// Canonical artifact name for one collected console log.
#[must_use]
pub fn console_artifact_name(job: &JobPath, build: BuildNumber) -> String {
    format!("jenkins-{}-build-{}.log", job.artifact_slug(), build.get())
}

pub struct DiscoverJobs;

impl Operation for DiscoverJobs {
    type Request = DiscoverJobsRequest;
    type Response = DiscoverJobsResponse;

    const EFFECT: OperationEffect = OperationEffect::ReadOnly;

    fn coordinates(_request: &Self::Request) -> OperationCoordinates {
        OperationCoordinates::new(capability(), server_resource())
    }
}

pub struct DiscoverBuilds;

impl Operation for DiscoverBuilds {
    type Request = DiscoverBuildsRequest;
    type Response = DiscoverBuildsResponse;

    const EFFECT: OperationEffect = OperationEffect::ReadOnly;

    fn coordinates(request: &Self::Request) -> OperationCoordinates {
        OperationCoordinates::new(capability(), request.job.resource())
    }
}

pub struct CollectConsoleLog;

impl Operation for CollectConsoleLog {
    type Request = CollectConsoleLogRequest;
    type Response = CollectConsoleLogResponse;

    const EFFECT: OperationEffect = OperationEffect::ReadOnly;

    fn coordinates(request: &Self::Request) -> OperationCoordinates {
        OperationCoordinates::new(capability(), request.job.build_resource(request.build))
    }
}

/// Minimal authenticated read-only probe used by connector self-tests.
///
/// It verifies base-URL reachability, TLS, and the stored credential by
/// requesting the authenticated user's own identity; no job data is read and
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
            CapabilityName::parse("connection.verify").expect("static Jenkins capability is valid"),
            ResourceName::parse("jenkins:api").expect("static Jenkins resource is valid"),
        )
    }
}

/// Jenkins connector over an injected bounded HTTP transport.
pub struct Jenkins<T> {
    api_base: Url,
    transport: T,
}

impl<T> Jenkins<T> {
    /// # Errors
    ///
    /// Returns an error unless the API base is a credential-free HTTPS hierarchy.
    pub fn new(api_base: Url, transport: T) -> Result<Self, InvalidJenkinsConfiguration> {
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
    pub fn with_base_str(
        api_base: &str,
        transport: T,
    ) -> Result<Self, InvalidJenkinsConfiguration> {
        let api_base = Url::parse(api_base).map_err(|_| InvalidJenkinsConfiguration)?;
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
            .map_err(|_| invalid_request("Jenkins API request path is invalid"))?;
        Ok(TransportRequest {
            url,
            authenticated: true,
            response_limit,
        })
    }
}

impl<T: JenkinsTransport> Connector<DiscoverJobs> for Jenkins<T> {
    fn descriptor(&self) -> ConnectorDescriptor {
        descriptor()
    }

    fn execute(
        &self,
        request: DiscoverJobsRequest,
        context: InvocationContext,
    ) -> ConnectorFuture<'_, crate::ConnectorResult<DiscoverJobsResponse>> {
        Box::pin(async move {
            context.preflight(OperationEffect::ReadOnly)?;
            request.validate()?;
            let path = format!("api/json?tree=jobs[name,url,color]{{0,{}}}", request.limit);
            let response = self
                .transport
                .get(self.api_request(&path, MAX_JSON_BYTES)?, &context)
                .await?;
            require_status(&response, StatusCode::OK)?;
            let mut envelope: JobsEnvelope = parse_json(&response.body)?;
            if envelope.jobs.len() > request.limit {
                return Err(remote_failure("Jenkins returned more jobs than requested"));
            }
            for job in &envelope.jobs {
                validate_discovered_job(job)?;
            }
            envelope
                .jobs
                .sort_by(|left, right| left.name.cmp(&right.name));
            let count = envelope.jobs.len();
            ConnectorOutput::new(
                DiscoverJobsResponse {
                    jobs: envelope.jobs,
                },
                summary(format!("found {count} Jenkins jobs"))?,
                Truth::Complete,
                Vec::new(),
                Vec::new(),
            )
            .map_err(|_| remote_failure("Jenkins job discovery output exceeded SDK bounds"))
        })
    }
}

impl<T: JenkinsTransport> Connector<DiscoverBuilds> for Jenkins<T> {
    fn descriptor(&self) -> ConnectorDescriptor {
        descriptor()
    }

    fn execute(
        &self,
        request: DiscoverBuildsRequest,
        context: InvocationContext,
    ) -> ConnectorFuture<'_, crate::ConnectorResult<DiscoverBuildsResponse>> {
        Box::pin(async move {
            context.preflight(OperationEffect::ReadOnly)?;
            request.validate()?;
            let path = format!(
                "{}/api/json?tree=builds[number,result,timestamp,duration,url]{{0,{}}}",
                request.job.api_path(),
                request.limit
            );
            let response = self
                .transport
                .get(self.api_request(&path, MAX_JSON_BYTES)?, &context)
                .await?;
            require_status(&response, StatusCode::OK)?;
            let mut envelope: BuildsEnvelope = parse_json(&response.body)?;
            if envelope.builds.len() > request.limit {
                return Err(remote_failure(
                    "Jenkins returned more builds than requested",
                ));
            }
            for build in &envelope.builds {
                validate_discovered_build(build)?;
            }
            envelope.builds.sort_by_key(|build| build.number);
            let count = envelope.builds.len();
            let job = request.job.as_coordinate();
            ConnectorOutput::new(
                DiscoverBuildsResponse {
                    job: job.clone(),
                    builds: envelope.builds,
                },
                summary(format!("found {count} Jenkins builds for {job}"))?,
                Truth::Complete,
                Vec::new(),
                Vec::new(),
            )
            .map_err(|_| remote_failure("Jenkins build discovery output exceeded SDK bounds"))
        })
    }
}

impl<T: JenkinsTransport> Connector<CollectConsoleLog> for Jenkins<T> {
    fn descriptor(&self) -> ConnectorDescriptor {
        descriptor()
    }

    fn execute(
        &self,
        request: CollectConsoleLogRequest,
        context: InvocationContext,
    ) -> ConnectorFuture<'_, crate::ConnectorResult<CollectConsoleLogResponse>> {
        Box::pin(async move {
            context.preflight(OperationEffect::ReadOnly)?;
            request.validate()?;
            let path = format!(
                "{}/{}/consoleText",
                request.job.api_path(),
                request.build.get()
            );
            let response = self
                .transport
                .get(self.api_request(&path, request.max_log_bytes)?, &context)
                .await?;
            require_status(&response, StatusCode::OK)?;
            let artifact_name = console_artifact_name(&request.job, request.build);
            let artifact = ExactArtifact::new(artifact_name.clone(), response.body)
                .map_err(|_| remote_failure("Jenkins console log exceeded SDK bounds"))?;
            let byte_len = artifact.bytes().len();
            let job = request.job.as_coordinate();
            let build = request.build.get();
            ConnectorOutput::new(
                CollectConsoleLogResponse {
                    job: job.clone(),
                    build: request.build,
                    artifact_name,
                    byte_len,
                },
                summary(format!(
                    "collected {byte_len} console log bytes for Jenkins build {job} #{build}"
                ))?,
                Truth::Complete,
                Vec::new(),
                vec![artifact],
            )
            .map_err(|_| remote_failure("Jenkins console log output exceeded SDK bounds"))
        })
    }
}

impl<T: JenkinsTransport> Connector<VerifyCredentials> for Jenkins<T> {
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
                .get(self.api_request("me/api/json", MAX_JSON_BYTES)?, &context)
                .await?;
            require_status(&response, StatusCode::OK)?;
            ConnectorOutput::new(
                VerifyCredentialsResponse {},
                summary("Jenkins credential and API base verified".to_owned())?,
                Truth::Complete,
                Vec::new(),
                Vec::new(),
            )
            .map_err(|_| remote_failure("Jenkins verification output exceeded SDK bounds"))
        })
    }
}

#[derive(Deserialize)]
struct JobsEnvelope {
    jobs: Vec<JenkinsJob>,
}

#[derive(Deserialize)]
struct BuildsEnvelope {
    builds: Vec<JenkinsBuild>,
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

pub trait JenkinsTransport: Send + Sync {
    fn get<'a>(
        &'a self,
        request: TransportRequest,
        context: &'a InvocationContext,
    ) -> ConnectorFuture<'a, Result<TransportResponse, ConnectorFailure>>;
}

impl<T: JenkinsTransport + ?Sized> JenkinsTransport for Arc<T> {
    fn get<'a>(
        &'a self,
        request: TransportRequest,
        context: &'a InvocationContext,
    ) -> ConnectorFuture<'a, Result<TransportResponse, ConnectorFailure>> {
        (**self).get(request, context)
    }
}

/// Production transport with native rustls verification, system proxy support, and no redirects.
pub struct ReqwestJenkinsTransport {
    client: reqwest::Client,
    credentials: Option<(String, String)>,
}

impl ReqwestJenkinsTransport {
    /// Accepts an optional combined `user:api-token` secret for HTTP Basic auth.
    ///
    /// # Errors
    ///
    /// Returns a sanitized error when the secret or secure HTTP client is invalid.
    pub fn new(secret: Option<String>) -> Result<Self, InvalidJenkinsConfiguration> {
        let credentials = secret.map(|secret| split_secret(&secret)).transpose()?;
        let client = reqwest::Client::builder()
            .use_rustls_tls()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| InvalidJenkinsConfiguration)?;
        Ok(Self {
            client,
            credentials,
        })
    }
}

fn split_secret(secret: &str) -> Result<(String, String), InvalidJenkinsConfiguration> {
    let (user, token) = secret.split_once(':').ok_or(InvalidJenkinsConfiguration)?;
    if user.is_empty()
        || token.is_empty()
        || user.len() > MAX_SECRET_PART_BYTES
        || token.len() > MAX_SECRET_PART_BYTES
        || user.chars().any(char::is_control)
        || token.chars().any(char::is_control)
    {
        return Err(InvalidJenkinsConfiguration);
    }
    Ok((user.to_owned(), token.to_owned()))
}

impl fmt::Debug for ReqwestJenkinsTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReqwestJenkinsTransport")
            .field("authenticated", &self.credentials.is_some())
            .finish_non_exhaustive()
    }
}

impl JenkinsTransport for ReqwestJenkinsTransport {
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
                && let Some((user, token)) = &self.credentials
            {
                builder = builder.basic_auth(user, Some(token));
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
pub struct InvalidJenkinsConfiguration;

impl fmt::Display for InvalidJenkinsConfiguration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Jenkins connector configuration is invalid")
    }
}

impl Error for InvalidJenkinsConfiguration {}

fn descriptor() -> ConnectorDescriptor {
    ConnectorDescriptor::new("jenkins", "json-api-1")
        .expect("static Jenkins connector descriptor is valid")
}

fn capability() -> CapabilityName {
    CapabilityName::parse("jobs.inspect").expect("static Jenkins capability is valid")
}

fn server_resource() -> ResourceName {
    ResourceName::parse("jenkins:server").expect("static Jenkins resource is valid")
}

fn summary(value: String) -> Result<BoundedSummary, ConnectorFailure> {
    BoundedSummary::new(value)
        .map_err(|_| remote_failure("Jenkins connector summary exceeded SDK bounds"))
}

fn parse_json<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, ConnectorFailure> {
    serde_json::from_slice(bytes)
        .map_err(|_| remote_failure("Jenkins returned malformed JSON metadata"))
}

fn require_status(
    response: &TransportResponse,
    expected: StatusCode,
) -> Result<(), ConnectorFailure> {
    if response.status == expected.as_u16() {
        return Ok(());
    }
    let message = safe_message("Jenkins API request failed");
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
            ConnectorFailure::certificate(safe_message("Jenkins certificate verification failed"))
        }
        FailureKind::Network => {
            ConnectorFailure::network(safe_message("Jenkins network connection failed"))
        }
        FailureKind::Remote => {
            ConnectorFailure::remote(safe_message("Jenkins HTTP request failed"), true)
        }
        _ => unreachable!("transport classifier returned a non-transport failure"),
    }
}

fn validate_discovered_job(job: &JenkinsJob) -> Result<(), ConnectorFailure> {
    if !valid_job_segment(&job.name)
        || !valid_https_text(&job.url)
        || job
            .color
            .as_deref()
            .is_some_and(|value| !valid_remote_text(value))
    {
        return Err(remote_failure("Jenkins job metadata was invalid"));
    }
    Ok(())
}

fn validate_discovered_build(build: &JenkinsBuild) -> Result<(), ConnectorFailure> {
    if !valid_https_text(&build.url)
        || build
            .result
            .as_deref()
            .is_some_and(|value| !valid_remote_text(value))
    {
        return Err(remote_failure("Jenkins build metadata was invalid"));
    }
    Ok(())
}

fn valid_remote_text(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_REMOTE_TEXT_BYTES
        && !value.chars().any(char::is_control)
}

fn valid_https_text(value: &str) -> bool {
    value.len() <= MAX_REMOTE_TEXT_BYTES
        && Url::parse(value)
            .ok()
            .is_some_and(|url| validate_https_url(&url).is_ok())
}

fn validate_https_url(url: &Url) -> Result<(), InvalidJenkinsConfiguration> {
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.cannot_be_a_base()
    {
        return Err(InvalidJenkinsConfiguration);
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

fn valid_job_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_JOB_SEGMENT_BYTES
        && !matches!(value, "." | "..")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn invalid_request(message: &str) -> ConnectorFailure {
    ConnectorFailure::invalid_request(safe_message(message))
}

fn remote_failure(message: &str) -> ConnectorFailure {
    ConnectorFailure::remote(safe_message(message), false)
}

fn safe_message(value: &str) -> FailureMessage {
    FailureMessage::new(value).expect("static Jenkins failure message is valid")
}
