use std::{
    collections::VecDeque,
    sync::Mutex,
    time::{Duration, Instant},
};

use url::Url;

use super::{
    CancellationToken, Connector, ConnectorDescriptor, FailureKind, InvocationContext, Operation,
    OperationCoordinates, RetryGuidance, Truth, verify_conformance,
};
use crate::{
    CapabilityName, ResourceName,
    sharepoint::{
        DiscoverDocuments, DiscoverDocumentsRequest, DiscoverLists, DiscoverListsRequest,
        MAX_SEARCH_QUERY_BYTES, MAX_SITE_ID_BYTES, ReqwestSharePointTransport, SearchQuery,
        SharePoint, SharePointTransport, SiteId, TransportRequest, TransportResponse,
        VerifyCredentials, VerifyCredentialsRequest,
    },
};

const SITE: &str = "contoso.example.test,3b2a1c0d-aaaa-bbbb-cccc-111122223333,9f8e7d6c-dddd-eeee-ffff-444455556666";

#[derive(Debug)]
struct SeenRequest {
    url: String,
    authenticated: bool,
    response_limit: usize,
}

enum Reply {
    Response(TransportResponse),
    Failure(super::ConnectorFailure),
}

struct FakeTransport {
    replies: Mutex<VecDeque<Reply>>,
    seen: Mutex<Vec<SeenRequest>>,
}

impl FakeTransport {
    fn new(replies: impl IntoIterator<Item = Reply>) -> Self {
        Self {
            replies: Mutex::new(replies.into_iter().collect()),
            seen: Mutex::new(Vec::new()),
        }
    }

    fn seen(&self) -> Vec<SeenRequest> {
        self.seen
            .lock()
            .expect("seen request lock must not be poisoned")
            .drain(..)
            .collect()
    }

    fn is_empty(&self) -> bool {
        self.replies
            .lock()
            .expect("reply lock must not be poisoned")
            .is_empty()
    }
}

impl SharePointTransport for FakeTransport {
    fn get<'a>(
        &'a self,
        request: TransportRequest,
        _context: &'a InvocationContext,
    ) -> super::ConnectorFuture<'a, Result<TransportResponse, super::ConnectorFailure>> {
        Box::pin(async move {
            self.seen
                .lock()
                .expect("seen request lock must not be poisoned")
                .push(SeenRequest {
                    url: request.url().as_str().to_owned(),
                    authenticated: request.authenticated(),
                    response_limit: request.response_limit(),
                });
            match self
                .replies
                .lock()
                .expect("reply lock must not be poisoned")
                .pop_front()
                .expect("fake transport must have a reply")
            {
                Reply::Response(response) => Ok(response),
                Reply::Failure(failure) => Err(failure),
            }
        })
    }
}

fn response(status: u16, body: impl Into<Vec<u8>>) -> Reply {
    Reply::Response(TransportResponse::new(status, Vec::new(), body.into()))
}

fn context() -> InvocationContext {
    InvocationContext::new(
        Instant::now() + Duration::from_mins(1),
        CancellationToken::new(),
        1,
        None,
    )
    .unwrap()
}

fn connector(transport: FakeTransport) -> SharePoint<FakeTransport> {
    SharePoint::new(
        Url::parse("https://graph.example.test/v1.0/").unwrap(),
        transport,
    )
    .unwrap()
}

fn documents_request(limit: usize) -> DiscoverDocumentsRequest {
    DiscoverDocumentsRequest::new(
        SiteId::parse(SITE).unwrap(),
        SearchQuery::parse("deploy runbook").unwrap(),
        limit,
    )
    .unwrap()
}

fn lists_request(limit: usize) -> DiscoverListsRequest {
    DiscoverListsRequest::new(SiteId::parse(SITE).unwrap(), limit).unwrap()
}

fn document_json(id: &str, name: &str) -> String {
    format!(
        r#"{{"id":"{id}","name":"{name}","webUrl":"https://contoso.example.test/doc/{id}","size":2048,"lastModifiedDateTime":"2026-08-01T10:00:00Z"}}"#
    )
}

fn list_json(id: &str, display_name: &str) -> String {
    format!(
        r#"{{"id":"{id}","displayName":"{display_name}","webUrl":"https://contoso.example.test/list/{id}"}}"#
    )
}

#[test]
fn sharepoint_id_and_query_bounds_and_policy_coordinates_are_exact() {
    let site = SiteId::parse(SITE).unwrap();
    assert_eq!(site.as_str(), SITE);
    assert_eq!(
        serde_json::to_string(&site).unwrap(),
        format!(r#""{SITE}""#)
    );
    assert_eq!(
        serde_json::from_str::<SiteId>(&format!(r#""{SITE}""#)).unwrap(),
        site
    );
    assert!(
        SiteId::parse("3b2a1c0d-aaaa-bbbb-cccc-111122223333").is_ok(),
        "a bare GUID site id must be accepted"
    );
    let too_long_site = "a".repeat(MAX_SITE_ID_BYTES + 1);
    for invalid in [
        "",
        "site id",
        "site/id",
        "site'id",
        "site\\id",
        "site\nid",
        "s\u{ed}te",
        &too_long_site,
    ] {
        assert!(
            SiteId::parse(invalid).is_err(),
            "must reject site id {invalid:?}"
        );
    }

    let query = SearchQuery::parse("deploy runbook").unwrap();
    assert_eq!(query.as_str(), "deploy runbook");
    let too_long_query = "q".repeat(MAX_SEARCH_QUERY_BYTES + 1);
    for invalid in ["", "query\nnewline", "quote'injection", &too_long_query] {
        assert!(
            SearchQuery::parse(invalid).is_err(),
            "must reject query {invalid:?}"
        );
    }

    assert!(
        DiscoverDocumentsRequest::new(site.clone(), query.clone(), 0).is_err(),
        "a zero document limit must be rejected"
    );
    assert!(
        DiscoverDocumentsRequest::new(site.clone(), query.clone(), 101).is_err(),
        "an over-limit document bound must be rejected"
    );
    assert!(
        DiscoverListsRequest::new(site.clone(), 0).is_err(),
        "a zero list limit must be rejected"
    );
    assert!(
        DiscoverListsRequest::new(site.clone(), 101).is_err(),
        "an over-limit list bound must be rejected"
    );

    let discover = DiscoverDocumentsRequest::new(site.clone(), query, 5).unwrap();
    let coordinates = DiscoverDocuments::coordinates(&discover);
    assert_eq!(coordinates.capability().as_str(), "documents.discover");
    assert_eq!(
        coordinates.resource().as_str(),
        format!("sharepoint:{SITE}/documents")
    );
    let lists = DiscoverListsRequest::new(site, 5).unwrap();
    let coordinates = DiscoverLists::coordinates(&lists);
    assert_eq!(coordinates.capability().as_str(), "lists.discover");
    assert_eq!(
        coordinates.resource().as_str(),
        format!("sharepoint:{SITE}/lists")
    );
}

#[tokio::test]
async fn sharepoint_operations_satisfy_the_connector_conformance_contract() {
    let descriptor = ConnectorDescriptor::new("sharepoint", "web-api-1").unwrap();

    verify_conformance::<_, DiscoverDocuments>(
        &connector(FakeTransport::new([])),
        documents_request(5),
        &descriptor,
        &OperationCoordinates::new(
            CapabilityName::parse("documents.discover").unwrap(),
            ResourceName::parse(format!("sharepoint:{SITE}/documents")).unwrap(),
        ),
    )
    .await
    .unwrap();
    verify_conformance::<_, DiscoverLists>(
        &connector(FakeTransport::new([])),
        lists_request(5),
        &descriptor,
        &OperationCoordinates::new(
            CapabilityName::parse("lists.discover").unwrap(),
            ResourceName::parse(format!("sharepoint:{SITE}/lists")).unwrap(),
        ),
    )
    .await
    .unwrap();
    verify_conformance::<_, VerifyCredentials>(
        &connector(FakeTransport::new([])),
        VerifyCredentialsRequest::default(),
        &descriptor,
        &OperationCoordinates::new(
            CapabilityName::parse("connection.verify").unwrap(),
            ResourceName::parse("sharepoint:api").unwrap(),
        ),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn document_search_is_encoded_bounded_sorted_and_partial_when_more_remain() {
    let body = format!(
        r#"{{"value":[{},{}]}}"#,
        document_json("zz-second", "beta.docx"),
        document_json("aa-first", "alpha.docx")
    );
    let connector = connector(FakeTransport::new([response(200, body)]));
    let output =
        Connector::<DiscoverDocuments>::execute(&connector, documents_request(3), context())
            .await
            .unwrap();
    assert!(output.truth().is_complete());
    assert_eq!(output.value().site(), SITE);
    assert_eq!(
        output
            .value()
            .documents()
            .iter()
            .map(crate::sharepoint::SharePointDocument::id)
            .collect::<Vec<_>>(),
        vec!["aa-first", "zz-second"]
    );
    assert_eq!(output.value().documents()[0].name(), "alpha.docx");
    assert_eq!(
        output.value().documents()[0].web_url(),
        "https://contoso.example.test/doc/aa-first"
    );
    assert_eq!(output.value().documents()[0].size(), 2048);
    assert_eq!(
        output.value().documents()[0].last_modified(),
        "2026-08-01T10:00:00Z"
    );
    let seen = connector.transport().seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].url,
        format!(
            "https://graph.example.test/v1.0/sites/{SITE}/drive/root/search(q='deploy%20runbook')?$top=3"
        )
    );
    assert!(seen[0].authenticated);
    assert_eq!(seen[0].response_limit, 1024 * 1024);
    assert!(connector.transport().is_empty());

    let next_link = format!(
        r#"{{"value":[{}],"@odata.nextLink":"https://graph.example.test/v1.0/next"}}"#,
        document_json("aa-first", "alpha.docx")
    );
    let paged = self::connector(FakeTransport::new([response(200, next_link)]));
    let output = Connector::<DiscoverDocuments>::execute(&paged, documents_request(3), context())
        .await
        .unwrap();
    let Truth::Partial { reason } = output.truth() else {
        panic!("a next link must make the search partial")
    };
    assert_eq!(
        reason.as_str(),
        "retained 1 matching documents; more may exist"
    );

    let capped = format!(
        r#"{{"value":[{},{}]}}"#,
        document_json("aa-first", "alpha.docx"),
        document_json("zz-second", "beta.docx")
    );
    let full = self::connector(FakeTransport::new([response(200, capped)]));
    let output = Connector::<DiscoverDocuments>::execute(&full, documents_request(2), context())
        .await
        .unwrap();
    assert!(
        matches!(output.truth(), Truth::Partial { .. }),
        "a listing that fills the requested bound must be partial"
    );

    let empty = self::connector(FakeTransport::new([response(200, r#"{"value":[]}"#)]));
    let output = Connector::<DiscoverDocuments>::execute(&empty, documents_request(5), context())
        .await
        .unwrap();
    assert!(output.truth().is_complete());
    assert!(output.value().documents().is_empty());

    let overflowing = format!(
        r#"{{"value":[{},{}]}}"#,
        document_json("aa-first", "alpha.docx"),
        document_json("zz-second", "beta.docx")
    );
    let strict = self::connector(FakeTransport::new([response(200, overflowing)]));
    let result =
        Connector::<DiscoverDocuments>::execute(&strict, documents_request(1), context()).await;
    let Err(failure) = result else {
        panic!("an over-limit document listing must fail")
    };
    assert_eq!(failure.kind(), FailureKind::Remote);

    let hostile = format!(
        r#"{{"value":[{}]}}"#,
        document_json("aa-first", "alpha\\ndocx")
    );
    let invalid = self::connector(FakeTransport::new([response(200, hostile)]));
    let Err(failure) =
        Connector::<DiscoverDocuments>::execute(&invalid, documents_request(3), context()).await
    else {
        panic!("hostile document metadata must fail")
    };
    assert_eq!(failure.kind(), FailureKind::Remote);
}

#[tokio::test]
async fn list_discovery_is_bounded_sorted_and_partial_when_more_remain() {
    let body = format!(
        r#"{{"value":[{},{}]}}"#,
        list_json("bbb", "Deployments"),
        list_json("aaa", "Assets")
    );
    let connector = connector(FakeTransport::new([response(200, body)]));
    let output = Connector::<DiscoverLists>::execute(&connector, lists_request(5), context())
        .await
        .unwrap();
    assert!(output.truth().is_complete());
    assert_eq!(output.value().site(), SITE);
    assert_eq!(
        output
            .value()
            .lists()
            .iter()
            .map(crate::sharepoint::SharePointList::id)
            .collect::<Vec<_>>(),
        vec!["aaa", "bbb"]
    );
    assert_eq!(output.value().lists()[0].display_name(), "Assets");
    assert_eq!(
        output.value().lists()[0].web_url(),
        "https://contoso.example.test/list/aaa"
    );
    let seen = connector.transport().seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].url,
        format!("https://graph.example.test/v1.0/sites/{SITE}/lists?$top=5")
    );
    assert!(seen[0].authenticated);
    assert_eq!(seen[0].response_limit, 1024 * 1024);
    assert!(connector.transport().is_empty());

    let next_link = format!(
        r#"{{"value":[{}],"@odata.nextLink":"https://graph.example.test/v1.0/next"}}"#,
        list_json("aaa", "Assets")
    );
    let paged = self::connector(FakeTransport::new([response(200, next_link)]));
    let output = Connector::<DiscoverLists>::execute(&paged, lists_request(5), context())
        .await
        .unwrap();
    let Truth::Partial { reason } = output.truth() else {
        panic!("a next link must make the listing partial")
    };
    assert_eq!(reason.as_str(), "retained 1 site lists; more may exist");

    let overflowing = format!(
        r#"{{"value":[{},{}]}}"#,
        list_json("aaa", "Assets"),
        list_json("bbb", "Deployments")
    );
    let strict = self::connector(FakeTransport::new([response(200, overflowing)]));
    let Err(failure) =
        Connector::<DiscoverLists>::execute(&strict, lists_request(1), context()).await
    else {
        panic!("an over-limit list listing must fail")
    };
    assert_eq!(failure.kind(), FailureKind::Remote);

    let hostile = format!(r#"{{"value":[{}]}}"#, list_json("aaa", "Assets\\u0007"));
    let invalid = self::connector(FakeTransport::new([response(200, hostile)]));
    let Err(failure) =
        Connector::<DiscoverLists>::execute(&invalid, lists_request(5), context()).await
    else {
        panic!("hostile list metadata must fail")
    };
    assert_eq!(failure.kind(), FailureKind::Remote);
}

#[tokio::test]
async fn oversized_responses_are_rejected_at_the_byte_cap() {
    let connector = connector(FakeTransport::new([Reply::Failure(
        super::ConnectorFailure::response_too_large(1024 * 1024),
    )]));
    let Err(failure) =
        Connector::<DiscoverDocuments>::execute(&connector, documents_request(5), context()).await
    else {
        panic!("an oversized response must fail")
    };
    assert_eq!(failure.kind(), FailureKind::ResponseTooLarge);
    assert_eq!(failure.response_limit(), Some(1024 * 1024));
    assert_eq!(failure.retry_guidance(), RetryGuidance::Never);
}

#[tokio::test]
async fn http_error_statuses_are_typed() {
    let cases: [(u16, FailureKind, RetryGuidance); 5] = [
        (
            401,
            FailureKind::Authentication,
            RetryGuidance::AfterConfigurationChange,
        ),
        (403, FailureKind::Forbidden, RetryGuidance::Never),
        (404, FailureKind::NotFound, RetryGuidance::Never),
        (
            500,
            FailureKind::Remote,
            RetryGuidance::AfterBackoff { delay: None },
        ),
        (418, FailureKind::Remote, RetryGuidance::Never),
    ];
    for (status, kind, retry) in cases {
        let connector = connector(FakeTransport::new([response(status, Vec::new())]));
        let Err(failure) =
            Connector::<DiscoverLists>::execute(&connector, lists_request(5), context()).await
        else {
            panic!("status {status} must fail")
        };
        assert_eq!(failure.kind(), kind);
        assert_eq!(failure.retry_guidance(), retry);
    }

    let rate_limited = Reply::Response(TransportResponse::new(
        429,
        vec![("Retry-After".to_owned(), "42".to_owned())],
        Vec::new(),
    ));
    let connector = connector(FakeTransport::new([rate_limited]));
    let Err(failure) =
        Connector::<DiscoverDocuments>::execute(&connector, documents_request(1), context()).await
    else {
        panic!("a rate-limited discovery must fail")
    };
    assert_eq!(failure.kind(), FailureKind::RateLimit);
    assert_eq!(
        failure.retry_guidance(),
        RetryGuidance::AfterBackoff {
            delay: Some(Duration::from_secs(42))
        }
    );
}

#[tokio::test]
async fn verify_credentials_requires_a_site_identity_body_not_just_http_200() {
    let sharepoint = connector(FakeTransport::new([response(
        200,
        format!(r#"{{"id":"{SITE}","displayName":"Contoso"}}"#),
    )]));
    let output = Connector::<VerifyCredentials>::execute(
        &sharepoint,
        VerifyCredentialsRequest::default(),
        context(),
    )
    .await
    .unwrap();
    let seen = sharepoint.transport().seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].url, "https://graph.example.test/v1.0/sites/root");
    assert!(seen[0].authenticated);
    assert!(output.truth().is_complete());

    for body in [
        "<html><body>sign in to continue</body></html>",
        "{}",
        r#"{"id":""}"#,
    ] {
        let anonymous = connector(FakeTransport::new([response(200, body)]));
        let Err(failure) = Connector::<VerifyCredentials>::execute(
            &anonymous,
            VerifyCredentialsRequest::default(),
            context(),
        )
        .await
        else {
            panic!("an identityless HTTP 200 body {body:?} must fail");
        };
        assert_eq!(failure.kind(), FailureKind::Authentication);
    }

    let unauthorized = connector(FakeTransport::new([response(401, "")]));
    let Err(failure) = Connector::<VerifyCredentials>::execute(
        &unauthorized,
        VerifyCredentialsRequest::default(),
        context(),
    )
    .await
    else {
        panic!("an unauthorized probe must fail");
    };
    assert_eq!(failure.kind(), FailureKind::Authentication);
}

#[test]
fn production_transport_debug_never_contains_the_token() {
    let token = "eyJ0eXAiOiJKV1QiLCJhbGciOiJSUzI1NiJ9.deadbeefpayload.signature";
    let transport = ReqwestSharePointTransport::new(Some(token.to_owned())).unwrap();
    let debug = format!("{transport:?}");
    assert!(debug.contains("authenticated"));
    assert!(!debug.contains("eyJ0eXAi"));
    assert!(!debug.contains("deadbeef"));
    for invalid in ["", "tok\nen", &"x".repeat(4097)] {
        assert!(
            ReqwestSharePointTransport::new(Some((*invalid).to_owned())).is_err(),
            "must reject {invalid:?}"
        );
    }
    assert!(
        SharePoint::new(
            Url::parse("http://graph.example.test/v1.0/").unwrap(),
            transport
        )
        .is_err(),
        "a plaintext HTTP base must be rejected"
    );
}
