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
    confluence::{
        CollectPage, CollectPageRequest, Confluence, ConfluenceTransport, Cql, DiscoverPages,
        DiscoverPagesRequest, MAX_BODY_BYTES, MAX_CQL_BYTES, MAX_DISCOVERED_PAGES,
        MAX_PAGE_ID_BYTES, MAX_SPACE_KEY_BYTES, PageId, ReqwestConfluenceTransport, SpaceKey,
        TransportRequest, TransportResponse, VerifyCredentials, VerifyCredentialsRequest,
    },
};

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

impl ConfluenceTransport for FakeTransport {
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

fn connector(transport: FakeTransport) -> Confluence<FakeTransport> {
    Confluence::new(
        Url::parse("https://team.example.test/wiki/").unwrap(),
        transport,
    )
    .unwrap()
}

fn discover_request(limit: usize) -> DiscoverPagesRequest {
    DiscoverPagesRequest::new(
        SpaceKey::parse("DOCS").unwrap(),
        Cql::parse("space = DOCS and type = page order by lastmodified desc").unwrap(),
        limit,
    )
    .unwrap()
}

fn search_page_json(id: &str, title: &str) -> String {
    format!(
        r#"{{"id":"{id}","title":"{title}","space":{{"key":"DOCS"}},"version":{{"number":3}}}}"#
    )
}

fn detail_page_json(id: &str, body: &str) -> String {
    format!(
        r#"{{"id":"{id}","title":"runbook","space":{{"key":"DOCS"}},"version":{{"number":7}},"body":{{"storage":{{"value":"{body}"}}}}}}"#
    )
}

#[test]
fn confluence_id_and_cql_bounds_and_policy_coordinates_are_exact() {
    let space = SpaceKey::parse("DOCS_1").unwrap();
    assert_eq!(space.as_str(), "DOCS_1");
    assert_eq!(serde_json::to_string(&space).unwrap(), r#""DOCS_1""#);
    assert_eq!(
        serde_json::from_str::<SpaceKey>(r#""DOCS_1""#).unwrap(),
        space
    );
    let too_long_space = "A".repeat(MAX_SPACE_KEY_BYTES + 1);
    for invalid in ["", "key space", "key-dash", "k\u{e9}y", &too_long_space] {
        assert!(
            SpaceKey::parse(invalid).is_err(),
            "must reject space key {invalid:?}"
        );
    }

    let page = PageId::parse("98342").unwrap();
    assert_eq!(page.as_str(), "98342");
    assert_eq!(serde_json::from_str::<PageId>(r#""98342""#).unwrap(), page);
    let too_long_page = "9".repeat(MAX_PAGE_ID_BYTES + 1);
    for invalid in ["", "98a42", "-98342", "9 8", &too_long_page] {
        assert!(
            PageId::parse(invalid).is_err(),
            "must reject page id {invalid:?}"
        );
    }

    let too_long_cql = "a".repeat(MAX_CQL_BYTES + 1);
    for invalid in ["", "cql\nnewline", &too_long_cql] {
        assert!(Cql::parse(invalid).is_err(), "must reject CQL {invalid:?}");
    }
    let cql = Cql::parse("space = DOCS").unwrap();
    assert_eq!(cql.as_str(), "space = DOCS");
    assert!(
        DiscoverPagesRequest::new(space.clone(), cql.clone(), 0).is_err(),
        "a zero limit must be rejected"
    );
    assert!(
        DiscoverPagesRequest::new(space.clone(), cql.clone(), MAX_DISCOVERED_PAGES + 1).is_err(),
        "an over-limit bound must be rejected"
    );

    let discover = DiscoverPagesRequest::new(space, cql, 5).unwrap();
    let coordinates = DiscoverPages::coordinates(&discover);
    assert_eq!(coordinates.capability().as_str(), "pages.inspect");
    assert_eq!(coordinates.resource().as_str(), "confluence:DOCS_1/pages");
    let collect = CollectPageRequest::new(page);
    let coordinates = CollectPage::coordinates(&collect);
    assert_eq!(coordinates.capability().as_str(), "pages.inspect");
    assert_eq!(coordinates.resource().as_str(), "confluence:98342");
}

#[tokio::test]
async fn confluence_operations_satisfy_the_connector_conformance_contract() {
    let descriptor = ConnectorDescriptor::new("confluence", "web-api-1").unwrap();
    let capability = CapabilityName::parse("pages.inspect").unwrap();

    verify_conformance::<_, DiscoverPages>(
        &connector(FakeTransport::new([])),
        discover_request(5),
        &descriptor,
        &OperationCoordinates::new(
            capability.clone(),
            ResourceName::parse("confluence:DOCS/pages").unwrap(),
        ),
    )
    .await
    .unwrap();
    verify_conformance::<_, CollectPage>(
        &connector(FakeTransport::new([])),
        CollectPageRequest::new(PageId::parse("98342").unwrap()),
        &descriptor,
        &OperationCoordinates::new(capability, ResourceName::parse("confluence:98342").unwrap()),
    )
    .await
    .unwrap();
    verify_conformance::<_, VerifyCredentials>(
        &connector(FakeTransport::new([])),
        VerifyCredentialsRequest::default(),
        &descriptor,
        &OperationCoordinates::new(
            CapabilityName::parse("connection.verify").unwrap(),
            ResourceName::parse("confluence:api").unwrap(),
        ),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn page_search_is_encoded_bounded_sorted_and_partial_when_more_remain() {
    let body = format!(
        r#"{{"results":[{},{}],"totalSize":2,"_links":{{}}}}"#,
        search_page_json("10", "second"),
        search_page_json("9", "first")
    );
    let connector = connector(FakeTransport::new([response(200, body)]));
    let output = Connector::<DiscoverPages>::execute(&connector, discover_request(2), context())
        .await
        .unwrap();
    assert!(output.truth().is_complete());
    assert_eq!(output.value().space(), "DOCS");
    assert_eq!(output.value().total(), 2);
    assert_eq!(
        output
            .value()
            .pages()
            .iter()
            .map(crate::confluence::ConfluencePage::id)
            .collect::<Vec<_>>(),
        vec!["9", "10"]
    );
    assert_eq!(output.value().pages()[0].title(), "first");
    assert_eq!(output.value().pages()[0].space(), "DOCS");
    assert_eq!(output.value().pages()[0].version(), 3);
    let seen = connector.transport().seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].url,
        "https://team.example.test/wiki/rest/api/content/search?cql=space+%3D+DOCS+and+type+%3D+page+order+by+lastmodified+desc&limit=2&expand=space%2Cversion"
    );
    assert!(seen[0].authenticated);
    assert_eq!(seen[0].response_limit, 1024 * 1024);
    assert!(connector.transport().is_empty());

    let truncated = format!(
        r#"{{"results":[{}],"totalSize":40}}"#,
        search_page_json("9", "first")
    );
    let partial = self::connector(FakeTransport::new([response(200, truncated)]));
    let output = Connector::<DiscoverPages>::execute(&partial, discover_request(1), context())
        .await
        .unwrap();
    let Truth::Partial { reason } = output.truth() else {
        panic!("a truncated search page must be partial")
    };
    assert_eq!(reason.as_str(), "retained 1 of 40 matching pages");

    let next_only = format!(
        r#"{{"results":[{}],"_links":{{"next":"/rest/api/content/search?cql=x&start=1"}}}}"#,
        search_page_json("9", "first")
    );
    let paged = self::connector(FakeTransport::new([response(200, next_only)]));
    let output = Connector::<DiscoverPages>::execute(&paged, discover_request(1), context())
        .await
        .unwrap();
    assert!(
        matches!(output.truth(), Truth::Partial { .. }),
        "a next link without a reported total must still be partial"
    );

    let empty = self::connector(FakeTransport::new([response(
        200,
        r#"{"results":[],"totalSize":0}"#,
    )]));
    let output = Connector::<DiscoverPages>::execute(&empty, discover_request(5), context())
        .await
        .unwrap();
    assert!(output.truth().is_complete());
    assert!(output.value().pages().is_empty());

    let overflowing = format!(
        r#"{{"results":[{},{}],"totalSize":2}}"#,
        search_page_json("9", "first"),
        search_page_json("10", "second")
    );
    let strict = self::connector(FakeTransport::new([response(200, overflowing)]));
    let result = Connector::<DiscoverPages>::execute(&strict, discover_request(1), context()).await;
    let Err(failure) = result else {
        panic!("an over-limit page listing must fail")
    };
    assert_eq!(failure.kind(), FailureKind::Remote);

    let hostile = format!(
        r#"{{"results":[{}],"totalSize":1}}"#,
        search_page_json("bad id", "first")
    );
    let invalid = self::connector(FakeTransport::new([response(200, hostile)]));
    let Err(failure) =
        Connector::<DiscoverPages>::execute(&invalid, discover_request(1), context()).await
    else {
        panic!("an invalid remote page id must fail")
    };
    assert_eq!(failure.kind(), FailureKind::Remote);
}

#[tokio::test]
async fn page_collection_returns_detail_and_truncates_long_bodies_as_partial() {
    let body = detail_page_json("98342", "<p>restart the daemon</p>");
    let connector = connector(FakeTransport::new([response(200, body)]));
    let request = CollectPageRequest::new(PageId::parse("98342").unwrap());
    let output = Connector::<CollectPage>::execute(&connector, request, context())
        .await
        .unwrap();
    assert!(output.truth().is_complete());
    assert_eq!(output.value().id(), "98342");
    assert_eq!(output.value().title(), "runbook");
    assert_eq!(output.value().space(), "DOCS");
    assert_eq!(output.value().version(), 7);
    assert_eq!(output.value().body(), Some("<p>restart the daemon</p>"));
    let seen = connector.transport().seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].url,
        "https://team.example.test/wiki/rest/api/content/98342?expand=body.storage%2Cspace%2Cversion"
    );
    assert!(seen[0].authenticated);
    assert!(connector.transport().is_empty());

    let oversized = "d".repeat(MAX_BODY_BYTES + 100);
    let long = self::connector(FakeTransport::new([response(
        200,
        detail_page_json("98342", &oversized),
    )]));
    let request = CollectPageRequest::new(PageId::parse("98342").unwrap());
    let output = Connector::<CollectPage>::execute(&long, request, context())
        .await
        .unwrap();
    let Truth::Partial { reason } = output.truth() else {
        panic!("a truncated body must be partial")
    };
    assert_eq!(
        reason.as_str(),
        format!("retained the first {MAX_BODY_BYTES} bytes of the page body")
    );
    assert_eq!(output.value().body().map(str::len), Some(MAX_BODY_BYTES));

    let missing = self::connector(FakeTransport::new([response(
        200,
        r#"{"id":"98342","title":"runbook","space":{"key":"DOCS"},"version":{"number":7}}"#,
    )]));
    let request = CollectPageRequest::new(PageId::parse("98342").unwrap());
    let output = Connector::<CollectPage>::execute(&missing, request, context())
        .await
        .unwrap();
    assert!(output.truth().is_complete());
    assert_eq!(output.value().body(), None);

    let hostile = self::connector(FakeTransport::new([response(
        200,
        r#"{"id":"98342","title":"run book","space":{"key":"DOCS"},"version":{"number":7}}"#,
    )]));
    let request = CollectPageRequest::new(PageId::parse("98342").unwrap());
    let Err(failure) = Connector::<CollectPage>::execute(&hostile, request, context()).await else {
        panic!("hostile page metadata must fail")
    };
    assert_eq!(failure.kind(), FailureKind::Remote);
}

#[tokio::test]
async fn oversized_responses_are_rejected_at_the_byte_cap() {
    let connector = connector(FakeTransport::new([Reply::Failure(
        super::ConnectorFailure::response_too_large(1024 * 1024),
    )]));
    let Err(failure) =
        Connector::<DiscoverPages>::execute(&connector, discover_request(5), context()).await
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
        let request = CollectPageRequest::new(PageId::parse("98342").unwrap());
        let Err(failure) = Connector::<CollectPage>::execute(&connector, request, context()).await
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
        Connector::<DiscoverPages>::execute(&connector, discover_request(1), context()).await
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
async fn verify_credentials_requires_an_identity_body_not_just_http_200() {
    let confluence = connector(FakeTransport::new([response(
        200,
        r#"{"accountId":"5b10ac8d82e05b22cc7d4ef5","displayName":"Jane Doe"}"#,
    )]));
    let output = Connector::<VerifyCredentials>::execute(
        &confluence,
        VerifyCredentialsRequest::default(),
        context(),
    )
    .await
    .unwrap();
    let seen = confluence.transport().seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].url,
        "https://team.example.test/wiki/rest/api/user/current"
    );
    assert!(seen[0].authenticated);
    assert!(output.truth().is_complete());

    for body in [
        "<html><body>log in to continue</body></html>",
        "{}",
        r#"{"accountId":"","displayName":""}"#,
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
fn production_transport_debug_never_contains_the_secret() {
    let secret = "jane@example.test:ATATTdeadbeefdeadbeef";
    let transport = ReqwestConfluenceTransport::new(Some(secret.to_owned())).unwrap();
    let debug = format!("{transport:?}");
    assert!(debug.contains("authenticated"));
    assert!(!debug.contains("jane@example.test"));
    assert!(!debug.contains("ATATT"));
    assert!(!debug.contains("deadbeef"));
    assert!(
        ReqwestConfluenceTransport::new(Some("jane@example.test:tok:en".to_owned())).is_ok(),
        "the API token half may contain colons"
    );
    for invalid in [
        "",
        "no-colon-token",
        ":leading",
        "trailing:",
        "user:tok\nen",
        &format!("jane@example.test:{}", "x".repeat(4097)),
    ] {
        assert!(
            ReqwestConfluenceTransport::new(Some((*invalid).to_owned())).is_err(),
            "must reject {invalid:?}"
        );
    }
    assert!(
        Confluence::new(
            Url::parse("http://team.example.test/wiki/").unwrap(),
            transport
        )
        .is_err(),
        "a plaintext HTTP base must be rejected"
    );
}
