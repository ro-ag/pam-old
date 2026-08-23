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
    jira::{
        CollectIssue, CollectIssueRequest, DiscoverIssues, DiscoverIssuesRequest, IssueKey, Jira,
        JiraTransport, Jql, MAX_DESCRIPTION_BYTES, MAX_DISCOVERED_ISSUES, MAX_ISSUE_KEY_BYTES,
        MAX_JQL_BYTES, MAX_PROJECT_KEY_BYTES, ProjectKey, ReqwestJiraTransport, TransportRequest,
        TransportResponse, VerifyCredentials, VerifyCredentialsRequest,
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

impl JiraTransport for FakeTransport {
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

fn connector(transport: FakeTransport) -> Jira<FakeTransport> {
    Jira::new(Url::parse("https://jira.example.test/").unwrap(), transport).unwrap()
}

fn discover_request(limit: usize) -> DiscoverIssuesRequest {
    DiscoverIssuesRequest::new(
        ProjectKey::parse("DEMO").unwrap(),
        Jql::parse("project = DEMO ORDER BY updated DESC").unwrap(),
        limit,
    )
    .unwrap()
}

fn search_issue_json(key: &str, summary: &str, status: &str) -> String {
    format!(
        r#"{{"key":"{key}","fields":{{"summary":"{summary}","status":{{"name":"{status}"}},"issuetype":{{"name":"Bug"}},"priority":{{"name":"High"}},"assignee":{{"displayName":"Jane Doe"}},"updated":"2026-08-01T10:00:00.000+0000"}}}}"#
    )
}

fn detail_issue_json(key: &str, description: &str) -> String {
    format!(
        r#"{{"key":"{key}","fields":{{"summary":"payment retries fail","description":"{description}","status":{{"name":"Open"}},"issuetype":{{"name":"Bug"}},"priority":{{"name":"High"}},"assignee":{{"displayName":"Jane Doe"}},"reporter":{{"displayName":"John Roe"}},"created":"2026-07-01T09:00:00.000+0000","updated":"2026-08-01T10:00:00.000+0000","labels":["billing"],"components":[{{"name":"payments"}}]}}}}"#
    )
}

#[test]
fn jira_key_and_jql_bounds_and_policy_coordinates_are_exact() {
    let project = ProjectKey::parse("DEMO_1").unwrap();
    assert_eq!(project.as_str(), "DEMO_1");
    assert_eq!(serde_json::to_string(&project).unwrap(), r#""DEMO_1""#);
    assert_eq!(
        serde_json::from_str::<ProjectKey>(r#""DEMO_1""#).unwrap(),
        project
    );
    let too_long_project = "A".repeat(MAX_PROJECT_KEY_BYTES + 1);
    for invalid in ["", "key space", "key-dash", "k\u{e9}y", &too_long_project] {
        assert!(
            ProjectKey::parse(invalid).is_err(),
            "must reject project key {invalid:?}"
        );
    }

    let issue = IssueKey::parse("DEMO-42").unwrap();
    assert_eq!(issue.as_str(), "DEMO-42");
    assert_eq!(
        serde_json::from_str::<IssueKey>(r#""DEMO-42""#).unwrap(),
        issue
    );
    let too_long_issue = "A".repeat(MAX_ISSUE_KEY_BYTES + 1);
    for invalid in ["", "key space", "key/slash", "k\u{e9}y", &too_long_issue] {
        assert!(
            IssueKey::parse(invalid).is_err(),
            "must reject issue key {invalid:?}"
        );
    }

    let too_long_jql = "a".repeat(MAX_JQL_BYTES + 1);
    for invalid in ["", "jql\nnewline", &too_long_jql] {
        assert!(Jql::parse(invalid).is_err(), "must reject JQL {invalid:?}");
    }
    let jql = Jql::parse("project = DEMO").unwrap();
    assert_eq!(jql.as_str(), "project = DEMO");
    assert!(
        DiscoverIssuesRequest::new(project.clone(), jql.clone(), 0).is_err(),
        "a zero limit must be rejected"
    );
    assert!(
        DiscoverIssuesRequest::new(project.clone(), jql.clone(), MAX_DISCOVERED_ISSUES + 1)
            .is_err(),
        "an over-limit bound must be rejected"
    );

    let discover = DiscoverIssuesRequest::new(project, jql, 5).unwrap();
    let coordinates = DiscoverIssues::coordinates(&discover);
    assert_eq!(coordinates.capability().as_str(), "issues.inspect");
    assert_eq!(coordinates.resource().as_str(), "jira:DEMO_1/issues");
    let collect = CollectIssueRequest::new(issue);
    let coordinates = CollectIssue::coordinates(&collect);
    assert_eq!(coordinates.capability().as_str(), "issues.inspect");
    assert_eq!(coordinates.resource().as_str(), "jira:DEMO-42");
}

#[tokio::test]
async fn jira_operations_satisfy_the_connector_conformance_contract() {
    let descriptor = ConnectorDescriptor::new("jira", "web-api-1").unwrap();
    let capability = CapabilityName::parse("issues.inspect").unwrap();

    verify_conformance::<_, DiscoverIssues>(
        &connector(FakeTransport::new([])),
        discover_request(5),
        &descriptor,
        &OperationCoordinates::new(
            capability.clone(),
            ResourceName::parse("jira:DEMO/issues").unwrap(),
        ),
    )
    .await
    .unwrap();
    verify_conformance::<_, CollectIssue>(
        &connector(FakeTransport::new([])),
        CollectIssueRequest::new(IssueKey::parse("DEMO-42").unwrap()),
        &descriptor,
        &OperationCoordinates::new(capability, ResourceName::parse("jira:DEMO-42").unwrap()),
    )
    .await
    .unwrap();
    verify_conformance::<_, VerifyCredentials>(
        &connector(FakeTransport::new([])),
        VerifyCredentialsRequest::default(),
        &descriptor,
        &OperationCoordinates::new(
            CapabilityName::parse("connection.verify").unwrap(),
            ResourceName::parse("jira:api").unwrap(),
        ),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn issue_search_is_encoded_bounded_sorted_and_partial_when_more_remain() {
    let body = format!(
        r#"{{"total":2,"issues":[{},{}]}}"#,
        search_issue_json("DEMO-2", "second", "Open"),
        search_issue_json("DEMO-1", "first", "Closed")
    );
    let connector = connector(FakeTransport::new([response(200, body)]));
    let output = Connector::<DiscoverIssues>::execute(&connector, discover_request(2), context())
        .await
        .unwrap();
    assert!(output.truth().is_complete());
    assert_eq!(output.value().project(), "DEMO");
    assert_eq!(output.value().total(), 2);
    assert_eq!(
        output
            .value()
            .issues()
            .iter()
            .map(crate::jira::JiraIssue::key)
            .collect::<Vec<_>>(),
        vec!["DEMO-1", "DEMO-2"]
    );
    assert_eq!(output.value().issues()[0].summary(), "first");
    assert_eq!(output.value().issues()[0].status(), "Closed");
    assert_eq!(output.value().issues()[0].issue_type(), "Bug");
    assert_eq!(output.value().issues()[0].priority(), Some("High"));
    assert_eq!(output.value().issues()[0].assignee(), Some("Jane Doe"));
    let seen = connector.transport().seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].url,
        "https://jira.example.test/rest/api/2/search?jql=project+%3D+DEMO+ORDER+BY+updated+DESC&maxResults=2&fields=summary%2Cstatus%2Cissuetype%2Cpriority%2Cassignee%2Cupdated"
    );
    assert!(seen[0].authenticated);
    assert_eq!(seen[0].response_limit, 1024 * 1024);
    assert!(connector.transport().is_empty());

    let truncated = format!(
        r#"{{"total":40,"issues":[{}]}}"#,
        search_issue_json("DEMO-1", "first", "Open")
    );
    let partial = self::connector(FakeTransport::new([response(200, truncated)]));
    let output = Connector::<DiscoverIssues>::execute(&partial, discover_request(1), context())
        .await
        .unwrap();
    let Truth::Partial { reason } = output.truth() else {
        panic!("a truncated search page must be partial")
    };
    assert_eq!(reason.as_str(), "retained 1 of 40 matching issues");

    let empty = self::connector(FakeTransport::new([response(
        200,
        r#"{"total":0,"issues":[]}"#,
    )]));
    let output = Connector::<DiscoverIssues>::execute(&empty, discover_request(5), context())
        .await
        .unwrap();
    assert!(output.truth().is_complete());
    assert!(output.value().issues().is_empty());

    let overflowing = format!(
        r#"{{"total":2,"issues":[{},{}]}}"#,
        search_issue_json("DEMO-1", "first", "Open"),
        search_issue_json("DEMO-2", "second", "Open")
    );
    let strict = self::connector(FakeTransport::new([response(200, overflowing)]));
    let result =
        Connector::<DiscoverIssues>::execute(&strict, discover_request(1), context()).await;
    let Err(failure) = result else {
        panic!("an over-limit issue listing must fail")
    };
    assert_eq!(failure.kind(), FailureKind::Remote);

    let hostile = format!(
        r#"{{"total":1,"issues":[{}]}}"#,
        search_issue_json("bad key", "first", "Open")
    );
    let invalid = self::connector(FakeTransport::new([response(200, hostile)]));
    let Err(failure) =
        Connector::<DiscoverIssues>::execute(&invalid, discover_request(1), context()).await
    else {
        panic!("an invalid remote issue key must fail")
    };
    assert_eq!(failure.kind(), FailureKind::Remote);
}

#[tokio::test]
async fn issue_collection_returns_detail_and_truncates_long_descriptions_as_partial() {
    let body = detail_issue_json("DEMO-42", "retries fail\\nafter one attempt");
    let connector = connector(FakeTransport::new([response(200, body)]));
    let request = CollectIssueRequest::new(IssueKey::parse("DEMO-42").unwrap());
    let output = Connector::<CollectIssue>::execute(&connector, request, context())
        .await
        .unwrap();
    assert!(output.truth().is_complete());
    assert_eq!(output.value().key(), "DEMO-42");
    assert_eq!(output.value().summary(), "payment retries fail");
    assert_eq!(
        output.value().description(),
        Some("retries fail\nafter one attempt")
    );
    assert_eq!(output.value().status(), "Open");
    assert_eq!(output.value().issue_type(), "Bug");
    assert_eq!(output.value().priority(), Some("High"));
    assert_eq!(output.value().assignee(), Some("Jane Doe"));
    assert_eq!(output.value().reporter(), Some("John Roe"));
    assert_eq!(output.value().created(), "2026-07-01T09:00:00.000+0000");
    assert_eq!(output.value().updated(), "2026-08-01T10:00:00.000+0000");
    assert_eq!(output.value().labels(), ["billing".to_owned()]);
    assert_eq!(output.value().components(), ["payments".to_owned()]);
    let seen = connector.transport().seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].url,
        "https://jira.example.test/rest/api/2/issue/DEMO-42?fields=summary%2Cdescription%2Cstatus%2Cissuetype%2Cpriority%2Cassignee%2Creporter%2Ccreated%2Cupdated%2Clabels%2Ccomponents"
    );
    assert!(seen[0].authenticated);
    assert!(connector.transport().is_empty());

    let oversized = "d".repeat(MAX_DESCRIPTION_BYTES + 100);
    let long = self::connector(FakeTransport::new([response(
        200,
        detail_issue_json("DEMO-42", &oversized),
    )]));
    let request = CollectIssueRequest::new(IssueKey::parse("DEMO-42").unwrap());
    let output = Connector::<CollectIssue>::execute(&long, request, context())
        .await
        .unwrap();
    let Truth::Partial { reason } = output.truth() else {
        panic!("a truncated description must be partial")
    };
    assert_eq!(
        reason.as_str(),
        format!("retained the first {MAX_DESCRIPTION_BYTES} bytes of the issue description")
    );
    assert_eq!(
        output.value().description().map(str::len),
        Some(MAX_DESCRIPTION_BYTES)
    );

    let missing = self::connector(FakeTransport::new([response(
        200,
        r#"{"key":"DEMO-42","fields":{"summary":"payment retries fail","status":{"name":"Open"},"issuetype":{"name":"Bug"},"created":"2026-07-01T09:00:00.000+0000","updated":"2026-08-01T10:00:00.000+0000"}}"#,
    )]));
    let request = CollectIssueRequest::new(IssueKey::parse("DEMO-42").unwrap());
    let output = Connector::<CollectIssue>::execute(&missing, request, context())
        .await
        .unwrap();
    assert!(output.truth().is_complete());
    assert_eq!(output.value().description(), None);
    assert_eq!(output.value().priority(), None);
    assert_eq!(output.value().assignee(), None);
    assert_eq!(output.value().reporter(), None);
    assert!(output.value().labels().is_empty());
    assert!(output.value().components().is_empty());
}

#[tokio::test]
async fn oversized_responses_are_rejected_at_the_byte_cap() {
    let connector = connector(FakeTransport::new([Reply::Failure(
        super::ConnectorFailure::response_too_large(1024 * 1024),
    )]));
    let Err(failure) =
        Connector::<DiscoverIssues>::execute(&connector, discover_request(5), context()).await
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
        let request = CollectIssueRequest::new(IssueKey::parse("DEMO-42").unwrap());
        let Err(failure) = Connector::<CollectIssue>::execute(&connector, request, context()).await
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
        Connector::<DiscoverIssues>::execute(&connector, discover_request(1), context()).await
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
    let jira = connector(FakeTransport::new([response(
        200,
        r#"{"name":"jdoe","key":"JIRAUSER10000"}"#,
    )]));
    let output = Connector::<VerifyCredentials>::execute(
        &jira,
        VerifyCredentialsRequest::default(),
        context(),
    )
    .await
    .unwrap();
    let seen = jira.transport().seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].url, "https://jira.example.test/rest/api/2/myself");
    assert!(seen[0].authenticated);
    assert!(output.truth().is_complete());

    for body in [
        "<html><body>log in to continue</body></html>",
        "{}",
        r#"{"name":"","key":""}"#,
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
    let token = "NjE0NTY6deadbeefdeadbeefdeadbeef";
    let transport = ReqwestJiraTransport::new(Some(token.to_owned())).unwrap();
    let debug = format!("{transport:?}");
    assert!(debug.contains("authenticated"));
    assert!(!debug.contains("NjE0NTY"));
    assert!(!debug.contains("deadbeef"));
    assert!(
        ReqwestJiraTransport::new(Some("with:colon".to_owned())).is_ok(),
        "a bearer token may contain colons"
    );
    for invalid in ["", "tok\nen", &"x".repeat(4097)] {
        assert!(
            ReqwestJiraTransport::new(Some((*invalid).to_owned())).is_err(),
            "must reject {invalid:?}"
        );
    }
    assert!(
        Jira::new(Url::parse("http://jira.example.test/").unwrap(), transport).is_err(),
        "a plaintext HTTP base must be rejected"
    );
}
