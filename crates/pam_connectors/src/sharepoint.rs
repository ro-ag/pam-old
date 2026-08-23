//! Typed `SharePoint` (Microsoft Graph) read-only operations.

use std::{collections::BTreeMap, error::Error, fmt, sync::Arc, time::Duration};

use reqwest::{StatusCode, header};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use url::Url;

use crate::{
    BoundedSummary, CapabilityName, Connector, ConnectorDescriptor, ConnectorFailure,
    ConnectorFuture, ConnectorOutput, FailureKind, FailureMessage, InvocationContext, Operation,
    OperationCoordinates, OperationEffect, ResourceName, Truth, github::classify_transport_failure,
};

pub const MAX_DISCOVERED_DOCUMENTS: usize = 100;
pub const MAX_DISCOVERED_LISTS: usize = 100;
pub const MAX_JSON_BYTES: usize = 1024 * 1024;
pub const MAX_SITE_ID_BYTES: usize = 512;
pub const MAX_SEARCH_QUERY_BYTES: usize = 1024;

const MAX_REMOTE_TEXT_BYTES: usize = 2048;
const MAX_SECRET_BYTES: usize = 4096;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// A validated Microsoft Graph site identity.
///
/// Graph accepts composite ids such as `contoso.sharepoint.com,{guid},{guid}`
/// as well as bare GUIDs, so the charset admits hostnames, commas, and the
/// parenthesized addressing forms while excluding separators that would alter
/// the request path.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct SiteId {
    id: String,
}

impl SiteId {
    /// Parses a bounded Graph site id.
    ///
    /// # Errors
    ///
    /// Returns an error unless the id is one to 512 bytes of ASCII
    /// alphanumerics or `-_.,:()`.
    pub fn parse(value: impl AsRef<str>) -> Result<Self, InvalidSharePointId> {
        let id = value.as_ref();
        if id.is_empty()
            || id.len() > MAX_SITE_ID_BYTES
            || !id.bytes().all(|byte| {
                byte.is_ascii_alphanumeric()
                    || matches!(byte, b'-' | b'_' | b'.' | b',' | b':' | b'(' | b')')
            })
        {
            return Err(InvalidSharePointId);
        }
        Ok(Self { id: id.to_owned() })
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.id
    }

    fn documents_resource(&self) -> ResourceName {
        ResourceName::parse(format!("sharepoint:{}/documents", self.id))
            .expect("validated SharePoint site coordinates fit the policy resource bound")
    }

    fn lists_resource(&self) -> ResourceName {
        ResourceName::parse(format!("sharepoint:{}/lists", self.id))
            .expect("validated SharePoint site coordinates fit the policy resource bound")
    }
}

impl fmt::Debug for SiteId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.id)
    }
}

impl<'de> Deserialize<'de> for SiteId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

impl Serialize for SiteId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.id)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidSharePointId;

impl fmt::Display for InvalidSharePointId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SharePoint site id must be a short bounded safe ASCII value")
    }
}

impl Error for InvalidSharePointId {}

/// A validated Graph drive search query.
///
/// Single quotes are rejected outright rather than escaped because the query
/// is embedded in the `search(q='...')` path form.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct SearchQuery {
    query: String,
}

impl SearchQuery {
    /// Parses a bounded search query.
    ///
    /// # Errors
    ///
    /// Returns an error unless the query is one to 1024 bytes without control
    /// characters or single quotes.
    pub fn parse(value: impl AsRef<str>) -> Result<Self, InvalidSearchQuery> {
        let query = value.as_ref();
        if query.is_empty()
            || query.len() > MAX_SEARCH_QUERY_BYTES
            || query.chars().any(char::is_control)
            || query.contains('\'')
        {
            return Err(InvalidSearchQuery);
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

impl fmt::Debug for SearchQuery {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.query)
    }
}

impl<'de> Deserialize<'de> for SearchQuery {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

impl Serialize for SearchQuery {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.query)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidSearchQuery;

impl fmt::Display for InvalidSearchQuery {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(
            "SharePoint search query must be one to 1024 bytes without control characters or \
             single quotes",
        )
    }
}

impl Error for InvalidSearchQuery {}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DiscoverDocumentsRequest {
    site: SiteId,
    query: SearchQuery,
    limit: usize,
}

impl DiscoverDocumentsRequest {
    /// # Errors
    ///
    /// Returns an error unless the limit is between one and 100.
    pub fn new(site: SiteId, query: SearchQuery, limit: usize) -> Result<Self, InvalidReadBound> {
        if !(1..=MAX_DISCOVERED_DOCUMENTS).contains(&limit) {
            return Err(InvalidReadBound);
        }
        Ok(Self { site, query, limit })
    }

    #[must_use]
    pub const fn site(&self) -> &SiteId {
        &self.site
    }

    #[must_use]
    pub const fn query(&self) -> &SearchQuery {
        &self.query
    }

    #[must_use]
    pub const fn limit(&self) -> usize {
        self.limit
    }

    fn validate(&self) -> Result<(), ConnectorFailure> {
        if (1..=MAX_DISCOVERED_DOCUMENTS).contains(&self.limit) {
            Ok(())
        } else {
            Err(invalid_request(
                "SharePoint document discovery limit is invalid",
            ))
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DiscoverListsRequest {
    site: SiteId,
    limit: usize,
}

impl DiscoverListsRequest {
    /// # Errors
    ///
    /// Returns an error unless the limit is between one and 100.
    pub fn new(site: SiteId, limit: usize) -> Result<Self, InvalidReadBound> {
        if !(1..=MAX_DISCOVERED_LISTS).contains(&limit) {
            return Err(InvalidReadBound);
        }
        Ok(Self { site, limit })
    }

    #[must_use]
    pub const fn site(&self) -> &SiteId {
        &self.site
    }

    #[must_use]
    pub const fn limit(&self) -> usize {
        self.limit
    }

    fn validate(&self) -> Result<(), ConnectorFailure> {
        if (1..=MAX_DISCOVERED_LISTS).contains(&self.limit) {
            Ok(())
        } else {
            Err(invalid_request(
                "SharePoint list discovery limit is invalid",
            ))
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidReadBound;

impl fmt::Display for InvalidReadBound {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SharePoint read limit exceeds the connector SDK bound")
    }
}

impl Error for InvalidReadBound {}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SharePointDocument {
    id: String,
    name: String,
    web_url: String,
    size: u64,
    last_modified: String,
}

impl SharePointDocument {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn web_url(&self) -> &str {
        &self.web_url
    }

    #[must_use]
    pub const fn size(&self) -> u64 {
        self.size
    }

    #[must_use]
    pub fn last_modified(&self) -> &str {
        &self.last_modified
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DiscoverDocumentsResponse {
    site: String,
    documents: Vec<SharePointDocument>,
}

impl DiscoverDocumentsResponse {
    #[must_use]
    pub fn site(&self) -> &str {
        &self.site
    }

    #[must_use]
    pub fn documents(&self) -> &[SharePointDocument] {
        &self.documents
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SharePointList {
    id: String,
    display_name: String,
    web_url: String,
}

impl SharePointList {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    #[must_use]
    pub fn web_url(&self) -> &str {
        &self.web_url
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DiscoverListsResponse {
    site: String,
    lists: Vec<SharePointList>,
}

impl DiscoverListsResponse {
    #[must_use]
    pub fn site(&self) -> &str {
        &self.site
    }

    #[must_use]
    pub fn lists(&self) -> &[SharePointList] {
        &self.lists
    }
}

pub struct DiscoverDocuments;

impl Operation for DiscoverDocuments {
    type Request = DiscoverDocumentsRequest;
    type Response = DiscoverDocumentsResponse;

    const EFFECT: OperationEffect = OperationEffect::ReadOnly;

    fn coordinates(request: &Self::Request) -> OperationCoordinates {
        OperationCoordinates::new(documents_capability(), request.site.documents_resource())
    }
}

pub struct DiscoverLists;

impl Operation for DiscoverLists {
    type Request = DiscoverListsRequest;
    type Response = DiscoverListsResponse;

    const EFFECT: OperationEffect = OperationEffect::ReadOnly;

    fn coordinates(request: &Self::Request) -> OperationCoordinates {
        OperationCoordinates::new(lists_capability(), request.site.lists_resource())
    }
}

/// Minimal authenticated read-only probe used by connector self-tests.
///
/// It verifies base-URL reachability, TLS, and the stored credential by
/// fetching the tenant root site; no document data is read and no remote
/// site details are returned.
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
                .expect("static SharePoint capability is valid"),
            ResourceName::parse("sharepoint:api").expect("static SharePoint resource is valid"),
        )
    }
}

/// `SharePoint` connector over an injected bounded HTTP transport.
pub struct SharePoint<T> {
    api_base: Url,
    transport: T,
}

impl<T> SharePoint<T> {
    /// # Errors
    ///
    /// Returns an error unless the API base is a credential-free HTTPS hierarchy.
    pub fn new(api_base: Url, transport: T) -> Result<Self, InvalidSharePointConfiguration> {
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
    ) -> Result<Self, InvalidSharePointConfiguration> {
        let api_base = Url::parse(api_base).map_err(|_| InvalidSharePointConfiguration)?;
        Self::new(api_base, transport)
    }

    #[must_use]
    pub const fn transport(&self) -> &T {
        &self.transport
    }

    /// Builds a Graph request from percent-encoded path segments and an
    /// optional `$top` bound.
    fn api_request(
        &self,
        segments: &[&str],
        top: Option<usize>,
        response_limit: usize,
    ) -> Result<TransportRequest, ConnectorFailure> {
        let mut url = self.api_base.clone();
        {
            let mut path = url
                .path_segments_mut()
                .map_err(|()| invalid_request("SharePoint API base cannot carry a path"))?;
            path.pop_if_empty();
            path.extend(segments);
        }
        if let Some(top) = top {
            url.set_query(Some(&format!("$top={top}")));
        }
        Ok(TransportRequest {
            url,
            authenticated: true,
            response_limit,
        })
    }
}

impl<T: SharePointTransport> Connector<DiscoverDocuments> for SharePoint<T> {
    fn descriptor(&self) -> ConnectorDescriptor {
        descriptor()
    }

    fn execute(
        &self,
        request: DiscoverDocumentsRequest,
        context: InvocationContext,
    ) -> ConnectorFuture<'_, crate::ConnectorResult<DiscoverDocumentsResponse>> {
        Box::pin(async move {
            context.preflight(OperationEffect::ReadOnly)?;
            request.validate()?;
            let search = format!("search(q='{}')", request.query.as_str());
            let transport_request = self.api_request(
                &["sites", request.site.as_str(), "drive", "root", &search],
                Some(request.limit),
                MAX_JSON_BYTES,
            )?;
            let response = self.transport.get(transport_request, &context).await?;
            require_status(&response, StatusCode::OK)?;
            let envelope: DriveSearchEnvelope = parse_json(&response.body)?;
            if envelope.value.len() > request.limit {
                return Err(remote_failure(
                    "SharePoint returned more documents than requested",
                ));
            }
            let mut documents = envelope
                .value
                .into_iter()
                .map(validated_document)
                .collect::<Result<Vec<_>, _>>()?;
            documents.sort_by(|left, right| left.id.cmp(&right.id));
            let count = documents.len();
            let site = request.site.as_str().to_owned();
            let truth = if envelope.next_link.is_some() || count == request.limit {
                Truth::Partial {
                    reason: summary(format!(
                        "retained {count} matching documents; more may exist"
                    ))?,
                }
            } else {
                Truth::Complete
            };
            ConnectorOutput::new(
                DiscoverDocumentsResponse {
                    site: site.clone(),
                    documents,
                },
                summary(format!("found {count} SharePoint documents for {site}"))?,
                truth,
                Vec::new(),
                Vec::new(),
            )
            .map_err(|_| remote_failure("SharePoint document discovery output exceeded SDK bounds"))
        })
    }
}

impl<T: SharePointTransport> Connector<DiscoverLists> for SharePoint<T> {
    fn descriptor(&self) -> ConnectorDescriptor {
        descriptor()
    }

    fn execute(
        &self,
        request: DiscoverListsRequest,
        context: InvocationContext,
    ) -> ConnectorFuture<'_, crate::ConnectorResult<DiscoverListsResponse>> {
        Box::pin(async move {
            context.preflight(OperationEffect::ReadOnly)?;
            request.validate()?;
            let transport_request = self.api_request(
                &["sites", request.site.as_str(), "lists"],
                Some(request.limit),
                MAX_JSON_BYTES,
            )?;
            let response = self.transport.get(transport_request, &context).await?;
            require_status(&response, StatusCode::OK)?;
            let envelope: SiteListsEnvelope = parse_json(&response.body)?;
            if envelope.value.len() > request.limit {
                return Err(remote_failure(
                    "SharePoint returned more lists than requested",
                ));
            }
            let mut lists = envelope
                .value
                .into_iter()
                .map(validated_list)
                .collect::<Result<Vec<_>, _>>()?;
            lists.sort_by(|left, right| left.id.cmp(&right.id));
            let count = lists.len();
            let site = request.site.as_str().to_owned();
            let truth = if envelope.next_link.is_some() || count == request.limit {
                Truth::Partial {
                    reason: summary(format!("retained {count} site lists; more may exist"))?,
                }
            } else {
                Truth::Complete
            };
            ConnectorOutput::new(
                DiscoverListsResponse {
                    site: site.clone(),
                    lists,
                },
                summary(format!("found {count} SharePoint lists for {site}"))?,
                truth,
                Vec::new(),
                Vec::new(),
            )
            .map_err(|_| remote_failure("SharePoint list discovery output exceeded SDK bounds"))
        })
    }
}

impl<T: SharePointTransport> Connector<VerifyCredentials> for SharePoint<T> {
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
                    self.api_request(&["sites", "root"], None, MAX_JSON_BYTES)?,
                    &context,
                )
                .await?;
            require_status(&response, StatusCode::OK)?;
            // An HTTP 200 alone is not acceptance: a proxy or portal can answer
            // an unauthenticated probe with an HTML page, so the body must name
            // a real Graph site identity.
            let identified = serde_json::from_slice::<SiteEnvelope>(&response.body)
                .ok()
                .and_then(|site| site.id)
                .is_some_and(|id| valid_remote_text(&id));
            if !identified {
                return Err(ConnectorFailure::authentication(safe_message(
                    "SharePoint did not acknowledge the stored credential with a Graph site \
                     identity",
                )));
            }
            ConnectorOutput::new(
                VerifyCredentialsResponse {},
                summary("SharePoint credential and API base verified".to_owned())?,
                Truth::Complete,
                Vec::new(),
                Vec::new(),
            )
            .map_err(|_| remote_failure("SharePoint verification output exceeded SDK bounds"))
        })
    }
}

#[derive(Deserialize)]
struct DriveSearchEnvelope {
    value: Vec<RawDriveItem>,
    #[serde(rename = "@odata.nextLink", default)]
    next_link: Option<String>,
}

#[derive(Deserialize)]
struct RawDriveItem {
    id: String,
    name: String,
    #[serde(rename = "webUrl")]
    web_url: String,
    size: u64,
    #[serde(rename = "lastModifiedDateTime")]
    last_modified: String,
}

#[derive(Deserialize)]
struct SiteListsEnvelope {
    value: Vec<RawSiteList>,
    #[serde(rename = "@odata.nextLink", default)]
    next_link: Option<String>,
}

#[derive(Deserialize)]
struct RawSiteList {
    id: String,
    #[serde(rename = "displayName")]
    display_name: String,
    #[serde(rename = "webUrl")]
    web_url: String,
}

#[derive(Deserialize)]
struct SiteEnvelope {
    #[serde(default)]
    id: Option<String>,
}

fn validated_document(raw: RawDriveItem) -> Result<SharePointDocument, ConnectorFailure> {
    if !valid_remote_text(&raw.id)
        || !valid_remote_text(&raw.name)
        || !valid_remote_text(&raw.web_url)
        || !valid_remote_text(&raw.last_modified)
    {
        return Err(remote_failure("SharePoint document metadata was invalid"));
    }
    Ok(SharePointDocument {
        id: raw.id,
        name: raw.name,
        web_url: raw.web_url,
        size: raw.size,
        last_modified: raw.last_modified,
    })
}

fn validated_list(raw: RawSiteList) -> Result<SharePointList, ConnectorFailure> {
    if !valid_remote_text(&raw.id)
        || !valid_remote_text(&raw.display_name)
        || !valid_remote_text(&raw.web_url)
    {
        return Err(remote_failure("SharePoint list metadata was invalid"));
    }
    Ok(SharePointList {
        id: raw.id,
        display_name: raw.display_name,
        web_url: raw.web_url,
    })
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

pub trait SharePointTransport: Send + Sync {
    fn get<'a>(
        &'a self,
        request: TransportRequest,
        context: &'a InvocationContext,
    ) -> ConnectorFuture<'a, Result<TransportResponse, ConnectorFailure>>;
}

impl<T: SharePointTransport + ?Sized> SharePointTransport for Arc<T> {
    fn get<'a>(
        &'a self,
        request: TransportRequest,
        context: &'a InvocationContext,
    ) -> ConnectorFuture<'a, Result<TransportResponse, ConnectorFailure>> {
        (**self).get(request, context)
    }
}

/// Production transport with native rustls verification, system proxy support, and no redirects.
pub struct ReqwestSharePointTransport {
    client: reqwest::Client,
    token: Option<String>,
}

impl ReqwestSharePointTransport {
    /// Accepts an optional Microsoft Graph access token sent as an HTTP
    /// `Bearer` credential.
    ///
    /// # Errors
    ///
    /// Returns a sanitized error when the token or secure HTTP client is invalid.
    pub fn new(token: Option<String>) -> Result<Self, InvalidSharePointConfiguration> {
        if let Some(token) = &token
            && !valid_token(token)
        {
            return Err(InvalidSharePointConfiguration);
        }
        let client = reqwest::Client::builder()
            .use_rustls_tls()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| InvalidSharePointConfiguration)?;
        Ok(Self { client, token })
    }
}

fn valid_token(token: &str) -> bool {
    !token.is_empty() && token.len() <= MAX_SECRET_BYTES && !token.chars().any(char::is_control)
}

impl fmt::Debug for ReqwestSharePointTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReqwestSharePointTransport")
            .field("authenticated", &self.token.is_some())
            .finish_non_exhaustive()
    }
}

impl SharePointTransport for ReqwestSharePointTransport {
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
pub struct InvalidSharePointConfiguration;

impl fmt::Display for InvalidSharePointConfiguration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SharePoint connector configuration is invalid")
    }
}

impl Error for InvalidSharePointConfiguration {}

fn descriptor() -> ConnectorDescriptor {
    ConnectorDescriptor::new("sharepoint", "web-api-1")
        .expect("static SharePoint connector descriptor is valid")
}

fn documents_capability() -> CapabilityName {
    CapabilityName::parse("documents.discover").expect("static SharePoint capability is valid")
}

fn lists_capability() -> CapabilityName {
    CapabilityName::parse("lists.discover").expect("static SharePoint capability is valid")
}

fn summary(value: String) -> Result<BoundedSummary, ConnectorFailure> {
    BoundedSummary::new(value)
        .map_err(|_| remote_failure("SharePoint connector summary exceeded SDK bounds"))
}

fn parse_json<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, ConnectorFailure> {
    serde_json::from_slice(bytes)
        .map_err(|_| remote_failure("SharePoint returned malformed JSON metadata"))
}

fn require_status(
    response: &TransportResponse,
    expected: StatusCode,
) -> Result<(), ConnectorFailure> {
    if response.status == expected.as_u16() {
        return Ok(());
    }
    let message = safe_message("SharePoint API request failed");
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
            "SharePoint certificate verification failed",
        )),
        FailureKind::Network => {
            ConnectorFailure::network(safe_message("SharePoint network connection failed"))
        }
        FailureKind::Remote => {
            ConnectorFailure::remote(safe_message("SharePoint HTTP request failed"), true)
        }
        _ => unreachable!("transport classifier returned a non-transport failure"),
    }
}

fn valid_remote_text(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_REMOTE_TEXT_BYTES
        && !value.chars().any(char::is_control)
}

fn validate_https_url(url: &Url) -> Result<(), InvalidSharePointConfiguration> {
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.cannot_be_a_base()
    {
        return Err(InvalidSharePointConfiguration);
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
    FailureMessage::new(value).expect("static SharePoint failure message is valid")
}
