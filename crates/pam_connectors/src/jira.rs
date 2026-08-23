//! Typed Jira Data Center operations.

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
pub const MAX_PROJECT_KEY_BYTES: usize = 64;
pub const MAX_ISSUE_KEY_BYTES: usize = 72;
pub const MAX_JQL_BYTES: usize = 2048;
pub const MAX_DESCRIPTION_BYTES: usize = 16 * 1024;

const MAX_REMOTE_TEXT_BYTES: usize = 2048;
const MAX_SECRET_BYTES: usize = 4096;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

const SEARCH_FIELDS: &str = "summary,status,issuetype,priority,assignee,updated";
const ISSUE_FIELDS: &str = "summary,description,status,issuetype,priority,assignee,reporter,created,updated,labels,components";

/// A validated Jira project key.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct ProjectKey {
    key: String,
}

impl ProjectKey {
    /// Parses a bounded Jira project key.
    ///
    /// # Errors
    ///
    /// Returns an error unless the key is one to 64 bytes of ASCII
    /// alphanumerics or `_`.
    pub fn parse(value: impl AsRef<str>) -> Result<Self, InvalidJiraKey> {
        let key = value.as_ref();
        if key.is_empty()
            || key.len() > MAX_PROJECT_KEY_BYTES
            || !key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err(InvalidJiraKey);
        }
        Ok(Self {
            key: key.to_owned(),
        })
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.key
    }

    fn issues_resource(&self) -> ResourceName {
        ResourceName::parse(format!("jira:{}/issues", self.key))
            .expect("validated Jira project coordinates fit the policy resource bound")
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

/// A validated Jira issue key such as `PROJ-42`.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct IssueKey {
    key: String,
}

impl IssueKey {
    /// Parses a bounded Jira issue key.
    ///
    /// # Errors
    ///
    /// Returns an error unless the key is one to 72 bytes of ASCII
    /// alphanumerics, `-`, or `_`.
    pub fn parse(value: impl AsRef<str>) -> Result<Self, InvalidJiraKey> {
        let key = value.as_ref();
        if key.is_empty()
            || key.len() > MAX_ISSUE_KEY_BYTES
            || !key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(InvalidJiraKey);
        }
        Ok(Self {
            key: key.to_owned(),
        })
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.key
    }

    fn resource(&self) -> ResourceName {
        ResourceName::parse(format!("jira:{}", self.key))
            .expect("validated Jira issue keys fit the policy resource bound")
    }
}

impl fmt::Debug for IssueKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.key)
    }
}

impl<'de> Deserialize<'de> for IssueKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

impl Serialize for IssueKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.key)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidJiraKey;

impl fmt::Display for InvalidJiraKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Jira key must be a short bounded safe ASCII identifier")
    }
}

impl Error for InvalidJiraKey {}

/// A validated JQL query string.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct Jql {
    query: String,
}

impl Jql {
    /// Parses a bounded JQL query.
    ///
    /// # Errors
    ///
    /// Returns an error unless the query is one to 2048 bytes without
    /// control characters.
    pub fn parse(value: impl AsRef<str>) -> Result<Self, InvalidJql> {
        let query = value.as_ref();
        if query.is_empty() || query.len() > MAX_JQL_BYTES || query.chars().any(char::is_control) {
            return Err(InvalidJql);
        }
        Ok(Self {
            query: query.to_owned(),
        })
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.query
    }
}

impl fmt::Debug for Jql {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.query)
    }
}

impl<'de> Deserialize<'de> for Jql {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

impl Serialize for Jql {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.query)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidJql;

impl fmt::Display for InvalidJql {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Jira JQL query must be one to 2048 bytes without control characters")
    }
}

impl Error for InvalidJql {}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DiscoverIssuesRequest {
    project: ProjectKey,
    jql: Jql,
    limit: usize,
}

impl DiscoverIssuesRequest {
    /// # Errors
    ///
    /// Returns an error unless the limit is between one and 100.
    pub fn new(project: ProjectKey, jql: Jql, limit: usize) -> Result<Self, InvalidReadBound> {
        if !(1..=MAX_DISCOVERED_ISSUES).contains(&limit) {
            return Err(InvalidReadBound);
        }
        Ok(Self {
            project,
            jql,
            limit,
        })
    }

    #[must_use]
    pub const fn project(&self) -> &ProjectKey {
        &self.project
    }

    #[must_use]
    pub const fn jql(&self) -> &Jql {
        &self.jql
    }

    #[must_use]
    pub const fn limit(&self) -> usize {
        self.limit
    }

    fn validate(&self) -> Result<(), ConnectorFailure> {
        if (1..=MAX_DISCOVERED_ISSUES).contains(&self.limit) {
            Ok(())
        } else {
            Err(invalid_request("Jira issue discovery limit is invalid"))
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidReadBound;

impl fmt::Display for InvalidReadBound {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Jira read limit exceeds the connector SDK bound")
    }
}

impl Error for InvalidReadBound {}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CollectIssueRequest {
    issue: IssueKey,
}

impl CollectIssueRequest {
    #[must_use]
    pub const fn new(issue: IssueKey) -> Self {
        Self { issue }
    }

    #[must_use]
    pub const fn issue(&self) -> &IssueKey {
        &self.issue
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct JiraIssue {
    key: String,
    summary: String,
    status: String,
    issue_type: String,
    priority: Option<String>,
    assignee: Option<String>,
    updated: String,
}

impl JiraIssue {
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    #[must_use]
    pub fn summary(&self) -> &str {
        &self.summary
    }

    #[must_use]
    pub fn status(&self) -> &str {
        &self.status
    }

    #[must_use]
    pub fn issue_type(&self) -> &str {
        &self.issue_type
    }

    #[must_use]
    pub fn priority(&self) -> Option<&str> {
        self.priority.as_deref()
    }

    #[must_use]
    pub fn assignee(&self) -> Option<&str> {
        self.assignee.as_deref()
    }

    #[must_use]
    pub fn updated(&self) -> &str {
        &self.updated
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DiscoverIssuesResponse {
    project: String,
    total: u64,
    issues: Vec<JiraIssue>,
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
    pub fn issues(&self) -> &[JiraIssue] {
        &self.issues
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CollectIssueResponse {
    key: String,
    summary: String,
    description: Option<String>,
    status: String,
    issue_type: String,
    priority: Option<String>,
    assignee: Option<String>,
    reporter: Option<String>,
    created: String,
    updated: String,
    labels: Vec<String>,
    components: Vec<String>,
}

impl CollectIssueResponse {
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    #[must_use]
    pub fn summary(&self) -> &str {
        &self.summary
    }

    #[must_use]
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    #[must_use]
    pub fn status(&self) -> &str {
        &self.status
    }

    #[must_use]
    pub fn issue_type(&self) -> &str {
        &self.issue_type
    }

    #[must_use]
    pub fn priority(&self) -> Option<&str> {
        self.priority.as_deref()
    }

    #[must_use]
    pub fn assignee(&self) -> Option<&str> {
        self.assignee.as_deref()
    }

    #[must_use]
    pub fn reporter(&self) -> Option<&str> {
        self.reporter.as_deref()
    }

    #[must_use]
    pub fn created(&self) -> &str {
        &self.created
    }

    #[must_use]
    pub fn updated(&self) -> &str {
        &self.updated
    }

    #[must_use]
    pub fn labels(&self) -> &[String] {
        &self.labels
    }

    #[must_use]
    pub fn components(&self) -> &[String] {
        &self.components
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

pub struct CollectIssue;

impl Operation for CollectIssue {
    type Request = CollectIssueRequest;
    type Response = CollectIssueResponse;

    const EFFECT: OperationEffect = OperationEffect::ReadOnly;

    fn coordinates(request: &Self::Request) -> OperationCoordinates {
        OperationCoordinates::new(capability(), request.issue.resource())
    }
}

/// Minimal authenticated read-only probe used by connector self-tests.
///
/// It verifies base-URL reachability, TLS, and the stored token by fetching
/// the authenticated identity; no issue data is read and no remote identity
/// details are returned.
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
            CapabilityName::parse("connection.verify").expect("static Jira capability is valid"),
            ResourceName::parse("jira:api").expect("static Jira resource is valid"),
        )
    }
}

/// Jira Data Center connector over an injected bounded HTTP transport.
pub struct Jira<T> {
    api_base: Url,
    transport: T,
}

impl<T> Jira<T> {
    /// # Errors
    ///
    /// Returns an error unless the API base is a credential-free HTTPS hierarchy.
    pub fn new(api_base: Url, transport: T) -> Result<Self, InvalidJiraConfiguration> {
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
    pub fn with_base_str(api_base: &str, transport: T) -> Result<Self, InvalidJiraConfiguration> {
        let api_base = Url::parse(api_base).map_err(|_| InvalidJiraConfiguration)?;
        Self::new(api_base, transport)
    }

    #[must_use]
    pub const fn transport(&self) -> &T {
        &self.transport
    }

    fn api_request(
        &self,
        path: &str,
        query: &[(&str, &str)],
        response_limit: usize,
    ) -> Result<TransportRequest, ConnectorFailure> {
        let mut url = self
            .api_base
            .join(path)
            .map_err(|_| invalid_request("Jira API request path is invalid"))?;
        if !query.is_empty() {
            let mut pairs = url.query_pairs_mut();
            for (name, value) in query {
                pairs.append_pair(name, value);
            }
        }
        Ok(TransportRequest {
            url,
            authenticated: true,
            response_limit,
        })
    }
}

impl<T: JiraTransport> Connector<DiscoverIssues> for Jira<T> {
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
            let limit_text = request.limit.to_string();
            let transport_request = self.api_request(
                "rest/api/2/search",
                &[
                    ("jql", request.jql.as_str()),
                    ("maxResults", &limit_text),
                    ("fields", SEARCH_FIELDS),
                ],
                MAX_JSON_BYTES,
            )?;
            let response = self.transport.get(transport_request, &context).await?;
            require_status(&response, StatusCode::OK)?;
            let envelope: SearchEnvelope = parse_json(&response.body)?;
            if envelope.issues.len() > request.limit {
                return Err(remote_failure("Jira returned more issues than requested"));
            }
            let mut issues = envelope
                .issues
                .into_iter()
                .map(validated_search_issue)
                .collect::<Result<Vec<_>, _>>()?;
            issues.sort_by(|left, right| left.key.cmp(&right.key));
            let count = issues.len();
            let retained = u64::try_from(count).expect("bounded issue count fits u64");
            let project = request.project.as_str().to_owned();
            let truth = if envelope.total > retained {
                Truth::Partial {
                    reason: summary(format!(
                        "retained {retained} of {} matching issues",
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
                    issues,
                },
                summary(format!("found {count} Jira issues for {project}"))?,
                truth,
                Vec::new(),
                Vec::new(),
            )
            .map_err(|_| remote_failure("Jira issue discovery output exceeded SDK bounds"))
        })
    }
}

impl<T: JiraTransport> Connector<CollectIssue> for Jira<T> {
    fn descriptor(&self) -> ConnectorDescriptor {
        descriptor()
    }

    fn execute(
        &self,
        request: CollectIssueRequest,
        context: InvocationContext,
    ) -> ConnectorFuture<'_, crate::ConnectorResult<CollectIssueResponse>> {
        Box::pin(async move {
            context.preflight(OperationEffect::ReadOnly)?;
            let path = format!("rest/api/2/issue/{}", request.issue.as_str());
            let transport_request =
                self.api_request(&path, &[("fields", ISSUE_FIELDS)], MAX_JSON_BYTES)?;
            let response = self.transport.get(transport_request, &context).await?;
            require_status(&response, StatusCode::OK)?;
            let envelope: IssueEnvelope = parse_json(&response.body)?;
            if IssueKey::parse(&envelope.key).is_err() {
                return Err(remote_failure("Jira issue key metadata was invalid"));
            }
            let fields = envelope.fields;
            let optional_valid = |value: Option<&str>| value.is_none_or(valid_remote_text);
            let priority = fields.priority.map(|value| value.name);
            let assignee = fields.assignee.map(|value| value.display_name);
            let reporter = fields.reporter.map(|value| value.display_name);
            if !valid_remote_text(&fields.summary)
                || !valid_remote_text(&fields.status.name)
                || !valid_remote_text(&fields.issuetype.name)
                || !valid_remote_text(&fields.created)
                || !valid_remote_text(&fields.updated)
                || !optional_valid(priority.as_deref())
                || !optional_valid(assignee.as_deref())
                || !optional_valid(reporter.as_deref())
                || !fields.labels.iter().all(|label| valid_remote_text(label))
                || !fields
                    .components
                    .iter()
                    .all(|component| valid_remote_text(&component.name))
            {
                return Err(remote_failure("Jira issue metadata was invalid"));
            }
            let (description, truncated) = bounded_description(fields.description)?;
            let key = envelope.key;
            let truth = if truncated {
                Truth::Partial {
                    reason: summary(format!(
                        "retained the first {MAX_DESCRIPTION_BYTES} bytes of the issue description"
                    ))?,
                }
            } else {
                Truth::Complete
            };
            let status = fields.status.name;
            ConnectorOutput::new(
                CollectIssueResponse {
                    key: key.clone(),
                    summary: fields.summary,
                    description,
                    status: status.clone(),
                    issue_type: fields.issuetype.name,
                    priority,
                    assignee,
                    reporter,
                    created: fields.created,
                    updated: fields.updated,
                    labels: fields.labels,
                    components: fields
                        .components
                        .into_iter()
                        .map(|component| component.name)
                        .collect(),
                },
                summary(format!("Jira issue {key} is {status}"))?,
                truth,
                Vec::new(),
                Vec::new(),
            )
            .map_err(|_| remote_failure("Jira issue collection output exceeded SDK bounds"))
        })
    }
}

impl<T: JiraTransport> Connector<VerifyCredentials> for Jira<T> {
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
                    self.api_request("rest/api/2/myself", &[], MAX_JSON_BYTES)?,
                    &context,
                )
                .await?;
            require_status(&response, StatusCode::OK)?;
            // An HTTP 200 alone is not acceptance: an unauthenticated Data
            // Center instance can answer with a login page, so the body must
            // be identity JSON naming the authenticated user.
            let identified = serde_json::from_slice::<MyselfEnvelope>(&response.body)
                .ok()
                .is_some_and(|myself| {
                    [myself.name, myself.key]
                        .into_iter()
                        .flatten()
                        .any(|value| valid_remote_text(&value))
                });
            if !identified {
                return Err(ConnectorFailure::authentication(safe_message(
                    "Jira did not acknowledge the stored token as an authenticated identity",
                )));
            }
            ConnectorOutput::new(
                VerifyCredentialsResponse {},
                summary("Jira token and API base verified".to_owned())?,
                Truth::Complete,
                Vec::new(),
                Vec::new(),
            )
            .map_err(|_| remote_failure("Jira verification output exceeded SDK bounds"))
        })
    }
}

#[derive(Deserialize)]
struct SearchEnvelope {
    total: u64,
    issues: Vec<RawSearchIssue>,
}

#[derive(Deserialize)]
struct RawSearchIssue {
    key: String,
    fields: RawSearchFields,
}

#[derive(Deserialize)]
struct RawSearchFields {
    summary: String,
    status: NamedField,
    issuetype: NamedField,
    #[serde(default)]
    priority: Option<NamedField>,
    #[serde(default)]
    assignee: Option<UserField>,
    updated: String,
}

#[derive(Deserialize)]
struct IssueEnvelope {
    key: String,
    fields: RawIssueFields,
}

#[derive(Deserialize)]
struct RawIssueFields {
    summary: String,
    #[serde(default)]
    description: Option<String>,
    status: NamedField,
    issuetype: NamedField,
    #[serde(default)]
    priority: Option<NamedField>,
    #[serde(default)]
    assignee: Option<UserField>,
    #[serde(default)]
    reporter: Option<UserField>,
    created: String,
    updated: String,
    #[serde(default)]
    labels: Vec<String>,
    #[serde(default)]
    components: Vec<NamedField>,
}

#[derive(Deserialize)]
struct NamedField {
    name: String,
}

#[derive(Deserialize)]
struct UserField {
    #[serde(rename = "displayName")]
    display_name: String,
}

#[derive(Deserialize)]
struct MyselfEnvelope {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    key: Option<String>,
}

fn validated_search_issue(raw: RawSearchIssue) -> Result<JiraIssue, ConnectorFailure> {
    let optional_valid = |value: Option<&str>| value.is_none_or(valid_remote_text);
    let priority = raw.fields.priority.map(|value| value.name);
    let assignee = raw.fields.assignee.map(|value| value.display_name);
    if IssueKey::parse(&raw.key).is_err()
        || !valid_remote_text(&raw.fields.summary)
        || !valid_remote_text(&raw.fields.status.name)
        || !valid_remote_text(&raw.fields.issuetype.name)
        || !valid_remote_text(&raw.fields.updated)
        || !optional_valid(priority.as_deref())
        || !optional_valid(assignee.as_deref())
    {
        return Err(remote_failure("Jira issue metadata was invalid"));
    }
    Ok(JiraIssue {
        key: raw.key,
        summary: raw.fields.summary,
        status: raw.fields.status.name,
        issue_type: raw.fields.issuetype.name,
        priority,
        assignee,
        updated: raw.fields.updated,
    })
}

/// Truncates a Jira description to the byte cap on a character boundary and
/// validates the retained text, which may span multiple lines.
fn bounded_description(
    description: Option<String>,
) -> Result<(Option<String>, bool), ConnectorFailure> {
    let Some(mut description) = description.filter(|value| !value.is_empty()) else {
        return Ok((None, false));
    };
    let truncated = description.len() > MAX_DESCRIPTION_BYTES;
    if truncated {
        let mut length = MAX_DESCRIPTION_BYTES;
        while !description.is_char_boundary(length) {
            length -= 1;
        }
        description.truncate(length);
    }
    if description
        .chars()
        .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(remote_failure("Jira issue description was invalid"));
    }
    Ok((Some(description), truncated))
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

pub trait JiraTransport: Send + Sync {
    fn get<'a>(
        &'a self,
        request: TransportRequest,
        context: &'a InvocationContext,
    ) -> ConnectorFuture<'a, Result<TransportResponse, ConnectorFailure>>;
}

impl<T: JiraTransport + ?Sized> JiraTransport for Arc<T> {
    fn get<'a>(
        &'a self,
        request: TransportRequest,
        context: &'a InvocationContext,
    ) -> ConnectorFuture<'a, Result<TransportResponse, ConnectorFailure>> {
        (**self).get(request, context)
    }
}

/// Production transport with native rustls verification, system proxy support, and no redirects.
pub struct ReqwestJiraTransport {
    client: reqwest::Client,
    token: Option<String>,
}

impl ReqwestJiraTransport {
    /// Accepts an optional Jira Data Center personal access token sent as an
    /// HTTP `Bearer` credential.
    ///
    /// # Errors
    ///
    /// Returns a sanitized error when the token or secure HTTP client is invalid.
    pub fn new(token: Option<String>) -> Result<Self, InvalidJiraConfiguration> {
        if let Some(token) = &token
            && !valid_token(token)
        {
            return Err(InvalidJiraConfiguration);
        }
        let client = reqwest::Client::builder()
            .use_rustls_tls()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| InvalidJiraConfiguration)?;
        Ok(Self { client, token })
    }
}

fn valid_token(token: &str) -> bool {
    !token.is_empty() && token.len() <= MAX_SECRET_BYTES && !token.chars().any(char::is_control)
}

impl fmt::Debug for ReqwestJiraTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReqwestJiraTransport")
            .field("authenticated", &self.token.is_some())
            .finish_non_exhaustive()
    }
}

impl JiraTransport for ReqwestJiraTransport {
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
                builder = builder.bearer_auth(token);
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
pub struct InvalidJiraConfiguration;

impl fmt::Display for InvalidJiraConfiguration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Jira connector configuration is invalid")
    }
}

impl Error for InvalidJiraConfiguration {}

fn descriptor() -> ConnectorDescriptor {
    ConnectorDescriptor::new("jira", "web-api-1")
        .expect("static Jira connector descriptor is valid")
}

fn capability() -> CapabilityName {
    CapabilityName::parse("issues.inspect").expect("static Jira capability is valid")
}

fn summary(value: String) -> Result<BoundedSummary, ConnectorFailure> {
    BoundedSummary::new(value)
        .map_err(|_| remote_failure("Jira connector summary exceeded SDK bounds"))
}

fn parse_json<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, ConnectorFailure> {
    serde_json::from_slice(bytes)
        .map_err(|_| remote_failure("Jira returned malformed JSON metadata"))
}

fn require_status(
    response: &TransportResponse,
    expected: StatusCode,
) -> Result<(), ConnectorFailure> {
    if response.status == expected.as_u16() {
        return Ok(());
    }
    let message = safe_message("Jira API request failed");
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
            ConnectorFailure::certificate(safe_message("Jira certificate verification failed"))
        }
        FailureKind::Network => {
            ConnectorFailure::network(safe_message("Jira network connection failed"))
        }
        FailureKind::Remote => {
            ConnectorFailure::remote(safe_message("Jira HTTP request failed"), true)
        }
        _ => unreachable!("transport classifier returned a non-transport failure"),
    }
}

fn valid_remote_text(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_REMOTE_TEXT_BYTES
        && !value.chars().any(char::is_control)
}

fn validate_https_url(url: &Url) -> Result<(), InvalidJiraConfiguration> {
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.cannot_be_a_base()
    {
        return Err(InvalidJiraConfiguration);
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
    FailureMessage::new(value).expect("static Jira failure message is valid")
}
