//! Typed Confluence Cloud operations.

use std::{collections::BTreeMap, error::Error, fmt, sync::Arc, time::Duration};

use reqwest::{StatusCode, header};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use url::Url;

use crate::{
    BoundedSummary, CapabilityName, Connector, ConnectorDescriptor, ConnectorFailure,
    ConnectorFuture, ConnectorOutput, FailureKind, FailureMessage, InvocationContext, Operation,
    OperationCoordinates, OperationEffect, ResourceName, Truth, github::classify_transport_failure,
};

pub const MAX_DISCOVERED_PAGES: usize = 100;
pub const MAX_JSON_BYTES: usize = 1024 * 1024;
pub const MAX_SPACE_KEY_BYTES: usize = 64;
pub const MAX_PAGE_ID_BYTES: usize = 32;
pub const MAX_CQL_BYTES: usize = 2048;
pub const MAX_BODY_BYTES: usize = 64 * 1024;

const MAX_REMOTE_TEXT_BYTES: usize = 2048;
const MAX_SECRET_BYTES: usize = 4096;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

const SEARCH_EXPAND: &str = "space,version";
const PAGE_EXPAND: &str = "body.storage,space,version";

/// A validated Confluence space key.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct SpaceKey {
    key: String,
}

impl SpaceKey {
    /// Parses a bounded Confluence space key.
    ///
    /// # Errors
    ///
    /// Returns an error unless the key is one to 64 bytes of ASCII
    /// alphanumerics or `_`.
    pub fn parse(value: impl AsRef<str>) -> Result<Self, InvalidConfluenceId> {
        let key = value.as_ref();
        if key.is_empty()
            || key.len() > MAX_SPACE_KEY_BYTES
            || !key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err(InvalidConfluenceId);
        }
        Ok(Self {
            key: key.to_owned(),
        })
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.key
    }

    fn pages_resource(&self) -> ResourceName {
        ResourceName::parse(format!("confluence:{}/pages", self.key))
            .expect("validated Confluence space coordinates fit the policy resource bound")
    }
}

impl fmt::Debug for SpaceKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.key)
    }
}

impl<'de> Deserialize<'de> for SpaceKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

impl Serialize for SpaceKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.key)
    }
}

/// A validated numeric Confluence content id such as `98342`.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct PageId {
    id: String,
}

impl PageId {
    /// Parses a bounded Confluence page id.
    ///
    /// # Errors
    ///
    /// Returns an error unless the id is one to 32 ASCII digits.
    pub fn parse(value: impl AsRef<str>) -> Result<Self, InvalidConfluenceId> {
        let id = value.as_ref();
        if id.is_empty()
            || id.len() > MAX_PAGE_ID_BYTES
            || !id.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(InvalidConfluenceId);
        }
        Ok(Self { id: id.to_owned() })
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.id
    }

    fn resource(&self) -> ResourceName {
        ResourceName::parse(format!("confluence:{}", self.id))
            .expect("validated Confluence page ids fit the policy resource bound")
    }
}

impl fmt::Debug for PageId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.id)
    }
}

impl<'de> Deserialize<'de> for PageId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

impl Serialize for PageId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.id)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidConfluenceId;

impl fmt::Display for InvalidConfluenceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Confluence identifier must be a short bounded safe ASCII value")
    }
}

impl Error for InvalidConfluenceId {}

/// A validated CQL query string.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct Cql {
    query: String,
}

impl Cql {
    /// Parses a bounded CQL query.
    ///
    /// # Errors
    ///
    /// Returns an error unless the query is one to 2048 bytes without
    /// control characters.
    pub fn parse(value: impl AsRef<str>) -> Result<Self, InvalidCql> {
        let query = value.as_ref();
        if query.is_empty() || query.len() > MAX_CQL_BYTES || query.chars().any(char::is_control) {
            return Err(InvalidCql);
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

impl fmt::Debug for Cql {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.query)
    }
}

impl<'de> Deserialize<'de> for Cql {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

impl Serialize for Cql {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.query)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidCql;

impl fmt::Display for InvalidCql {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .write_str("Confluence CQL query must be one to 2048 bytes without control characters")
    }
}

impl Error for InvalidCql {}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DiscoverPagesRequest {
    space: SpaceKey,
    cql: Cql,
    limit: usize,
}

impl DiscoverPagesRequest {
    /// # Errors
    ///
    /// Returns an error unless the limit is between one and 100.
    pub fn new(space: SpaceKey, cql: Cql, limit: usize) -> Result<Self, InvalidReadBound> {
        if !(1..=MAX_DISCOVERED_PAGES).contains(&limit) {
            return Err(InvalidReadBound);
        }
        Ok(Self { space, cql, limit })
    }

    #[must_use]
    pub const fn space(&self) -> &SpaceKey {
        &self.space
    }

    #[must_use]
    pub const fn cql(&self) -> &Cql {
        &self.cql
    }

    #[must_use]
    pub const fn limit(&self) -> usize {
        self.limit
    }

    fn validate(&self) -> Result<(), ConnectorFailure> {
        if (1..=MAX_DISCOVERED_PAGES).contains(&self.limit) {
            Ok(())
        } else {
            Err(invalid_request(
                "Confluence page discovery limit is invalid",
            ))
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidReadBound;

impl fmt::Display for InvalidReadBound {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Confluence read limit exceeds the connector SDK bound")
    }
}

impl Error for InvalidReadBound {}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CollectPageRequest {
    page: PageId,
}

impl CollectPageRequest {
    #[must_use]
    pub const fn new(page: PageId) -> Self {
        Self { page }
    }

    #[must_use]
    pub const fn page(&self) -> &PageId {
        &self.page
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ConfluencePage {
    id: String,
    title: String,
    space: String,
    version: u64,
}

impl ConfluencePage {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    #[must_use]
    pub fn space(&self) -> &str {
        &self.space
    }

    #[must_use]
    pub const fn version(&self) -> u64 {
        self.version
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DiscoverPagesResponse {
    space: String,
    total: u64,
    pages: Vec<ConfluencePage>,
}

impl DiscoverPagesResponse {
    #[must_use]
    pub fn space(&self) -> &str {
        &self.space
    }

    #[must_use]
    pub const fn total(&self) -> u64 {
        self.total
    }

    #[must_use]
    pub fn pages(&self) -> &[ConfluencePage] {
        &self.pages
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CollectPageResponse {
    id: String,
    title: String,
    space: String,
    version: u64,
    body: Option<String>,
}

impl CollectPageResponse {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    #[must_use]
    pub fn space(&self) -> &str {
        &self.space
    }

    #[must_use]
    pub const fn version(&self) -> u64 {
        self.version
    }

    #[must_use]
    pub fn body(&self) -> Option<&str> {
        self.body.as_deref()
    }
}

pub struct DiscoverPages;

impl Operation for DiscoverPages {
    type Request = DiscoverPagesRequest;
    type Response = DiscoverPagesResponse;

    const EFFECT: OperationEffect = OperationEffect::ReadOnly;

    fn coordinates(request: &Self::Request) -> OperationCoordinates {
        OperationCoordinates::new(capability(), request.space.pages_resource())
    }
}

pub struct CollectPage;

impl Operation for CollectPage {
    type Request = CollectPageRequest;
    type Response = CollectPageResponse;

    const EFFECT: OperationEffect = OperationEffect::ReadOnly;

    fn coordinates(request: &Self::Request) -> OperationCoordinates {
        OperationCoordinates::new(capability(), request.page.resource())
    }
}

/// Minimal authenticated read-only probe used by connector self-tests.
///
/// It verifies base-URL reachability, TLS, and the stored credential by
/// fetching the authenticated identity; no page data is read and no remote
/// identity details are returned.
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
                .expect("static Confluence capability is valid"),
            ResourceName::parse("confluence:api").expect("static Confluence resource is valid"),
        )
    }
}

/// Confluence Cloud connector over an injected bounded HTTP transport.
pub struct Confluence<T> {
    api_base: Url,
    transport: T,
}

impl<T> Confluence<T> {
    /// # Errors
    ///
    /// Returns an error unless the API base is a credential-free HTTPS hierarchy.
    pub fn new(api_base: Url, transport: T) -> Result<Self, InvalidConfluenceConfiguration> {
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
    ) -> Result<Self, InvalidConfluenceConfiguration> {
        let api_base = Url::parse(api_base).map_err(|_| InvalidConfluenceConfiguration)?;
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
            .map_err(|_| invalid_request("Confluence API request path is invalid"))?;
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

impl<T: ConfluenceTransport> Connector<DiscoverPages> for Confluence<T> {
    fn descriptor(&self) -> ConnectorDescriptor {
        descriptor()
    }

    fn execute(
        &self,
        request: DiscoverPagesRequest,
        context: InvocationContext,
    ) -> ConnectorFuture<'_, crate::ConnectorResult<DiscoverPagesResponse>> {
        Box::pin(async move {
            context.preflight(OperationEffect::ReadOnly)?;
            request.validate()?;
            let limit_text = request.limit.to_string();
            let transport_request = self.api_request(
                "rest/api/content/search",
                &[
                    ("cql", request.cql.as_str()),
                    ("limit", &limit_text),
                    ("expand", SEARCH_EXPAND),
                ],
                MAX_JSON_BYTES,
            )?;
            let response = self.transport.get(transport_request, &context).await?;
            require_status(&response, StatusCode::OK)?;
            let envelope: SearchEnvelope = parse_json(&response.body)?;
            if envelope.results.len() > request.limit {
                return Err(remote_failure(
                    "Confluence returned more pages than requested",
                ));
            }
            let mut pages = envelope
                .results
                .into_iter()
                .map(validated_search_page)
                .collect::<Result<Vec<_>, _>>()?;
            // Digit-only ids without a length tiebreak would sort "9" after "10".
            pages
                .sort_by(|left, right| (left.id.len(), &left.id).cmp(&(right.id.len(), &right.id)));
            let count = pages.len();
            let retained = u64::try_from(count).expect("bounded page count fits u64");
            let space = request.space.as_str().to_owned();
            let has_next = envelope.links.next.is_some();
            let total = envelope.total_size.unwrap_or(retained).max(retained);
            let truth = if total > retained || (envelope.total_size.is_none() && has_next) {
                Truth::Partial {
                    reason: summary(format!("retained {retained} of {total} matching pages"))?,
                }
            } else {
                Truth::Complete
            };
            ConnectorOutput::new(
                DiscoverPagesResponse {
                    space: space.clone(),
                    total,
                    pages,
                },
                summary(format!("found {count} Confluence pages for {space}"))?,
                truth,
                Vec::new(),
                Vec::new(),
            )
            .map_err(|_| remote_failure("Confluence page discovery output exceeded SDK bounds"))
        })
    }
}

impl<T: ConfluenceTransport> Connector<CollectPage> for Confluence<T> {
    fn descriptor(&self) -> ConnectorDescriptor {
        descriptor()
    }

    fn execute(
        &self,
        request: CollectPageRequest,
        context: InvocationContext,
    ) -> ConnectorFuture<'_, crate::ConnectorResult<CollectPageResponse>> {
        Box::pin(async move {
            context.preflight(OperationEffect::ReadOnly)?;
            let path = format!("rest/api/content/{}", request.page.as_str());
            let transport_request =
                self.api_request(&path, &[("expand", PAGE_EXPAND)], MAX_JSON_BYTES)?;
            let response = self.transport.get(transport_request, &context).await?;
            require_status(&response, StatusCode::OK)?;
            let envelope: PageEnvelope = parse_json(&response.body)?;
            if PageId::parse(&envelope.id).is_err()
                || !valid_remote_text(&envelope.title)
                || !valid_remote_text(&envelope.space.key)
            {
                return Err(remote_failure("Confluence page metadata was invalid"));
            }
            let (body, truncated) = bounded_body(
                envelope
                    .body
                    .and_then(|body| body.storage)
                    .map(|storage| storage.value),
            )?;
            let truth = if truncated {
                Truth::Partial {
                    reason: summary(format!(
                        "retained the first {MAX_BODY_BYTES} bytes of the page body"
                    ))?,
                }
            } else {
                Truth::Complete
            };
            let id = envelope.id;
            let version = envelope.version.number;
            ConnectorOutput::new(
                CollectPageResponse {
                    id: id.clone(),
                    title: envelope.title,
                    space: envelope.space.key,
                    version,
                    body,
                },
                summary(format!(
                    "collected Confluence page {id} at version {version}"
                ))?,
                truth,
                Vec::new(),
                Vec::new(),
            )
            .map_err(|_| remote_failure("Confluence page collection output exceeded SDK bounds"))
        })
    }
}

impl<T: ConfluenceTransport> Connector<VerifyCredentials> for Confluence<T> {
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
                    self.api_request("rest/api/user/current", &[], MAX_JSON_BYTES)?,
                    &context,
                )
                .await?;
            require_status(&response, StatusCode::OK)?;
            // An HTTP 200 alone is not acceptance: Confluence can answer an
            // unauthenticated probe with anonymous-user JSON or a login page,
            // so the body must name an authenticated identity.
            let identified = serde_json::from_slice::<CurrentUserEnvelope>(&response.body)
                .ok()
                .is_some_and(|user| {
                    [user.account_id, user.display_name]
                        .into_iter()
                        .flatten()
                        .any(|value| valid_remote_text(&value))
                });
            if !identified {
                return Err(ConnectorFailure::authentication(safe_message(
                    "Confluence did not acknowledge the stored credential as an authenticated \
                     identity",
                )));
            }
            ConnectorOutput::new(
                VerifyCredentialsResponse {},
                summary("Confluence credential and API base verified".to_owned())?,
                Truth::Complete,
                Vec::new(),
                Vec::new(),
            )
            .map_err(|_| remote_failure("Confluence verification output exceeded SDK bounds"))
        })
    }
}

#[derive(Deserialize)]
struct SearchEnvelope {
    results: Vec<RawSearchPage>,
    #[serde(rename = "totalSize", default)]
    total_size: Option<u64>,
    #[serde(rename = "_links", default)]
    links: SearchLinks,
}

#[derive(Default, Deserialize)]
struct SearchLinks {
    #[serde(default)]
    next: Option<String>,
}

#[derive(Deserialize)]
struct RawSearchPage {
    id: String,
    title: String,
    space: SpaceField,
    version: VersionField,
}

#[derive(Deserialize)]
struct PageEnvelope {
    id: String,
    title: String,
    space: SpaceField,
    version: VersionField,
    #[serde(default)]
    body: Option<BodyField>,
}

#[derive(Deserialize)]
struct SpaceField {
    key: String,
}

#[derive(Deserialize)]
struct VersionField {
    number: u64,
}

#[derive(Deserialize)]
struct BodyField {
    #[serde(default)]
    storage: Option<StorageField>,
}

#[derive(Deserialize)]
struct StorageField {
    value: String,
}

#[derive(Deserialize)]
struct CurrentUserEnvelope {
    #[serde(rename = "accountId", default)]
    account_id: Option<String>,
    #[serde(rename = "displayName", default)]
    display_name: Option<String>,
}

fn validated_search_page(raw: RawSearchPage) -> Result<ConfluencePage, ConnectorFailure> {
    if PageId::parse(&raw.id).is_err()
        || !valid_remote_text(&raw.title)
        || !valid_remote_text(&raw.space.key)
    {
        return Err(remote_failure("Confluence page metadata was invalid"));
    }
    Ok(ConfluencePage {
        id: raw.id,
        title: raw.title,
        space: raw.space.key,
        version: raw.version.number,
    })
}

/// Truncates a Confluence storage body to the byte cap on a character boundary
/// and validates the retained text, which may span multiple lines.
fn bounded_body(body: Option<String>) -> Result<(Option<String>, bool), ConnectorFailure> {
    let Some(mut body) = body.filter(|value| !value.is_empty()) else {
        return Ok((None, false));
    };
    let truncated = body.len() > MAX_BODY_BYTES;
    if truncated {
        let mut length = MAX_BODY_BYTES;
        while !body.is_char_boundary(length) {
            length -= 1;
        }
        body.truncate(length);
    }
    if body
        .chars()
        .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(remote_failure("Confluence page body was invalid"));
    }
    Ok((Some(body), truncated))
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

pub trait ConfluenceTransport: Send + Sync {
    fn get<'a>(
        &'a self,
        request: TransportRequest,
        context: &'a InvocationContext,
    ) -> ConnectorFuture<'a, Result<TransportResponse, ConnectorFailure>>;
}

impl<T: ConfluenceTransport + ?Sized> ConfluenceTransport for Arc<T> {
    fn get<'a>(
        &'a self,
        request: TransportRequest,
        context: &'a InvocationContext,
    ) -> ConnectorFuture<'a, Result<TransportResponse, ConnectorFailure>> {
        (**self).get(request, context)
    }
}

/// Production transport with native rustls verification, system proxy support, and no redirects.
pub struct ReqwestConfluenceTransport {
    client: reqwest::Client,
    credentials: Option<(String, String)>,
}

impl ReqwestConfluenceTransport {
    /// Accepts an optional combined `email:api-token` secret for HTTP Basic auth.
    ///
    /// # Errors
    ///
    /// Returns a sanitized error when the secret or secure HTTP client is invalid.
    pub fn new(secret: Option<String>) -> Result<Self, InvalidConfluenceConfiguration> {
        let credentials = secret.map(|secret| split_secret(&secret)).transpose()?;
        let client = reqwest::Client::builder()
            .use_rustls_tls()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| InvalidConfluenceConfiguration)?;
        Ok(Self {
            client,
            credentials,
        })
    }
}

fn split_secret(secret: &str) -> Result<(String, String), InvalidConfluenceConfiguration> {
    if secret.len() > MAX_SECRET_BYTES {
        return Err(InvalidConfluenceConfiguration);
    }
    let (email, token) = secret
        .split_once(':')
        .ok_or(InvalidConfluenceConfiguration)?;
    if email.is_empty()
        || token.is_empty()
        || email.chars().any(char::is_control)
        || token.chars().any(char::is_control)
    {
        return Err(InvalidConfluenceConfiguration);
    }
    Ok((email.to_owned(), token.to_owned()))
}

impl fmt::Debug for ReqwestConfluenceTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReqwestConfluenceTransport")
            .field("authenticated", &self.credentials.is_some())
            .finish_non_exhaustive()
    }
}

impl ConfluenceTransport for ReqwestConfluenceTransport {
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
                && let Some((email, token)) = &self.credentials
            {
                builder = builder.basic_auth(email, Some(token));
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
pub struct InvalidConfluenceConfiguration;

impl fmt::Display for InvalidConfluenceConfiguration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Confluence connector configuration is invalid")
    }
}

impl Error for InvalidConfluenceConfiguration {}

fn descriptor() -> ConnectorDescriptor {
    ConnectorDescriptor::new("confluence", "web-api-1")
        .expect("static Confluence connector descriptor is valid")
}

fn capability() -> CapabilityName {
    CapabilityName::parse("pages.inspect").expect("static Confluence capability is valid")
}

fn summary(value: String) -> Result<BoundedSummary, ConnectorFailure> {
    BoundedSummary::new(value)
        .map_err(|_| remote_failure("Confluence connector summary exceeded SDK bounds"))
}

fn parse_json<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, ConnectorFailure> {
    serde_json::from_slice(bytes)
        .map_err(|_| remote_failure("Confluence returned malformed JSON metadata"))
}

fn require_status(
    response: &TransportResponse,
    expected: StatusCode,
) -> Result<(), ConnectorFailure> {
    if response.status == expected.as_u16() {
        return Ok(());
    }
    let message = safe_message("Confluence API request failed");
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
        FailureKind::Certificate => ConnectorFailure::certificate(safe_message(
            "Confluence certificate verification failed",
        )),
        FailureKind::Network => {
            ConnectorFailure::network(safe_message("Confluence network connection failed"))
        }
        FailureKind::Remote => {
            ConnectorFailure::remote(safe_message("Confluence HTTP request failed"), true)
        }
        _ => unreachable!("transport classifier returned a non-transport failure"),
    }
}

fn valid_remote_text(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_REMOTE_TEXT_BYTES
        && !value.chars().any(char::is_control)
}

fn validate_https_url(url: &Url) -> Result<(), InvalidConfluenceConfiguration> {
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.cannot_be_a_base()
    {
        return Err(InvalidConfluenceConfiguration);
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
    FailureMessage::new(value).expect("static Confluence failure message is valid")
}
