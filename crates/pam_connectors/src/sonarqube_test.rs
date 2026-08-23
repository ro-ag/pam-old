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
    sonarqube::{
        DiscoverIssues, DiscoverIssuesRequest, FetchQualityGate, FetchQualityGateRequest,
        MAX_DISCOVERED_ISSUES, MAX_PROJECT_KEY_BYTES, ProjectKey, ReqwestSonarTransport, SonarQube,
        SonarTransport, TransportRequest, TransportResponse, VerifyCredentials,
        VerifyCredentialsRequest,
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

impl SonarTransport for FakeTransport {
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

fn connector(transport: FakeTransport) -> SonarQube<FakeTransport> {
    SonarQube::new(
        Url::parse("https://sonarqube.example.test/").unwrap(),
        transport,
    )
    .unwrap()
}

fn issue_json(key: &str, rule: &str, severity: &str, component: &str) -> String {
    format!(
        r#"{{"key":"{key}","rule":"{rule}","severity":"{severity}","component":"{component}","line":5,"message":"fix {rule}","type":"CODE_SMELL"}}"#
    )
}

#[test]
fn project_key_bounds_and_policy_coordinates_are_exact() {
    let project = ProjectKey::parse("org:app").unwrap();
    assert_eq!(project.as_str(), "org:app");
    assert_eq!(project.artifact_slug(), "org-app");
    assert_eq!(serde_json::to_string(&project).unwrap(), r#""org:app""#);
    assert_eq!(
        serde_json::from_str::<ProjectKey>(r#""org:app""#).unwrap(),
        project
    );
    let too_long = "a".repeat(MAX_PROJECT_KEY_BYTES + 1);
    for invalid in [
        "",
        "key with space",
        "key/slash",
        "k\u{e9}y",
        too_long.as_str(),
    ] {
        assert!(
            ProjectKey::parse(invalid).is_err(),
            "must reject {invalid:?}"
        );
    }
    assert!(DiscoverIssuesRequest::new(project.clone(), 0).is_err());
    assert!(DiscoverIssuesRequest::new(project.clone(), MAX_DISCOVERED_ISSUES + 1).is_err());

    let gate = FetchQualityGateRequest::new(project.clone());
    let coordinates = FetchQualityGate::coordinates(&gate);
    assert_eq!(coordinates.capability().as_str(), "projects.inspect");
    assert_eq!(coordinates.resource().as_str(), "sonarqube:org:app");
    let issues = DiscoverIssuesRequest::new(project, 5).unwrap();
    assert_eq!(
        DiscoverIssues::coordinates(&issues).resource().as_str(),
        "sonarqube:org:app/issues"
    );
}

#[tokio::test]
async fn sonarqube_operations_satisfy_the_connector_conformance_contract() {
    let descriptor = ConnectorDescriptor::new("sonarqube", "web-api-1").unwrap();
    let capability = CapabilityName::parse("projects.inspect").unwrap();
    let project = ProjectKey::parse("demo").unwrap();

    verify_conformance::<_, FetchQualityGate>(
        &connector(FakeTransport::new([])),
        FetchQualityGateRequest::new(project.clone()),
        &descriptor,
        &OperationCoordinates::new(
            capability.clone(),
            ResourceName::parse("sonarqube:demo").unwrap(),
        ),
    )
    .await
    .unwrap();
    verify_conformance::<_, DiscoverIssues>(
        &connector(FakeTransport::new([])),
        DiscoverIssuesRequest::new(project, 5).unwrap(),
        &descriptor,
        &OperationCoordinates::new(
            capability,
            ResourceName::parse("sonarqube:demo/issues").unwrap(),
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
            ResourceName::parse("sonarqube:api").unwrap(),
        ),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn failing_quality_gates_retain_only_failed_conditions_with_thresholds() {
    let body = r#"{"projectStatus":{"status":"ERROR","conditions":[
        {"status":"ERROR","metricKey":"new_coverage","comparator":"LT","errorThreshold":"85","actualValue":"62.5"},
        {"status":"OK","metricKey":"new_bugs","comparator":"GT","errorThreshold":"0","actualValue":"0"}
    ]}}"#;
    let connector = connector(FakeTransport::new([response(200, body)]));
    let request = FetchQualityGateRequest::new(ProjectKey::parse("org:app").unwrap());
    let output = Connector::<FetchQualityGate>::execute(&connector, request, context())
        .await
        .unwrap();
    assert!(output.truth().is_complete());
    assert_eq!(output.value().project(), "org:app");
    assert_eq!(output.value().status(), "ERROR");
    assert!(!output.value().is_passing());
    assert_eq!(output.value().failed_conditions().len(), 1);
    let condition = &output.value().failed_conditions()[0];
    assert_eq!(condition.metric_key(), "new_coverage");
    assert_eq!(condition.status(), "ERROR");
    assert_eq!(condition.comparator(), Some("LT"));
    assert_eq!(condition.error_threshold(), Some("85"));
    assert_eq!(condition.actual_value(), Some("62.5"));
    let seen = connector.transport().seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].url,
        "https://sonarqube.example.test/api/qualitygates/project_status?projectKey=org:app"
    );
    assert!(seen[0].authenticated);
    assert_eq!(seen[0].response_limit, 1024 * 1024);
    assert!(connector.transport().is_empty());

    let passing = self::connector(FakeTransport::new([response(
        200,
        r#"{"projectStatus":{"status":"OK","conditions":[]}}"#,
    )]));
    let request = FetchQualityGateRequest::new(ProjectKey::parse("org:app").unwrap());
    let output = Connector::<FetchQualityGate>::execute(&passing, request, context())
        .await
        .unwrap();
    assert!(output.value().is_passing());
    assert!(output.value().failed_conditions().is_empty());
}

#[tokio::test]
async fn issue_discovery_is_bounded_sorted_and_partial_when_more_remain() {
    let body = format!(
        r#"{{"total":2,"issues":[{},{}]}}"#,
        issue_json("issue-b", "java:S2", "MAJOR", "demo:src/B.java"),
        issue_json("issue-a", "java:S1", "MINOR", "demo:src/A.java")
    );
    let connector = connector(FakeTransport::new([response(200, body)]));
    let request = DiscoverIssuesRequest::new(ProjectKey::parse("demo").unwrap(), 2).unwrap();
    let output = Connector::<DiscoverIssues>::execute(&connector, request, context())
        .await
        .unwrap();
    assert!(output.truth().is_complete());
    assert_eq!(output.value().project(), "demo");
    assert_eq!(output.value().total(), 2);
    assert_eq!(
        output
            .value()
            .issues()
            .iter()
            .map(crate::sonarqube::SonarIssue::key)
            .collect::<Vec<_>>(),
        vec!["issue-a", "issue-b"]
    );
    assert_eq!(output.value().issues()[0].rule(), "java:S1");
    assert_eq!(output.value().issues()[0].severity(), "MINOR");
    assert_eq!(output.value().issues()[0].issue_type(), "CODE_SMELL");
    assert_eq!(output.value().issues()[0].line(), Some(5));
    let seen = connector.transport().seen();
    assert_eq!(
        seen[0].url,
        "https://sonarqube.example.test/api/issues/search?componentKeys=demo&resolved=false&ps=2"
    );
    assert!(seen[0].authenticated);
    assert!(connector.transport().is_empty());

    let truncated = format!(
        r#"{{"total":40,"issues":[{}]}}"#,
        issue_json("issue-a", "java:S1", "MINOR", "demo:src/A.java")
    );
    let partial = self::connector(FakeTransport::new([response(200, truncated)]));
    let request = DiscoverIssuesRequest::new(ProjectKey::parse("demo").unwrap(), 1).unwrap();
    let output = Connector::<DiscoverIssues>::execute(&partial, request, context())
        .await
        .unwrap();
    let Truth::Partial { reason } = output.truth() else {
        panic!("a truncated issue page must be partial")
    };
    assert_eq!(reason.as_str(), "retained 1 of 40 unresolved issues");

    let empty = self::connector(FakeTransport::new([response(
        200,
        r#"{"total":0,"issues":[]}"#,
    )]));
    let request = DiscoverIssuesRequest::new(ProjectKey::parse("demo").unwrap(), 5).unwrap();
    let output = Connector::<DiscoverIssues>::execute(&empty, request, context())
        .await
        .unwrap();
    assert!(output.truth().is_complete());
    assert!(output.value().issues().is_empty());

    let overflowing = format!(
        r#"{{"total":2,"issues":[{},{}]}}"#,
        issue_json("issue-a", "java:S1", "MINOR", "demo:src/A.java"),
        issue_json("issue-b", "java:S2", "MAJOR", "demo:src/B.java")
    );
    let strict = self::connector(FakeTransport::new([response(200, overflowing)]));
    let request = DiscoverIssuesRequest::new(ProjectKey::parse("demo").unwrap(), 1).unwrap();
    let result = Connector::<DiscoverIssues>::execute(&strict, request, context()).await;
    let Err(failure) = result else {
        panic!("an over-limit issue listing must fail")
    };
    assert_eq!(failure.kind(), FailureKind::Remote);
}

#[tokio::test]
async fn oversized_responses_are_rejected_at_the_byte_cap() {
    let connector = connector(FakeTransport::new([Reply::Failure(
        super::ConnectorFailure::response_too_large(1024 * 1024),
    )]));
    let request = DiscoverIssuesRequest::new(ProjectKey::parse("demo").unwrap(), 5).unwrap();
    let Err(failure) = Connector::<DiscoverIssues>::execute(&connector, request, context()).await
    else {
        panic!("an oversized response must fail")
    };
    assert_eq!(failure.kind(), FailureKind::ResponseTooLarge);
    assert_eq!(failure.response_limit(), Some(1024 * 1024));
    assert_eq!(failure.retry_guidance(), RetryGuidance::Never);
}

#[tokio::test]
async fn http_error_statuses_are_typed() {
    let cases: [(u16, FailureKind, RetryGuidance); 4] = [
        (
            401,
            FailureKind::Authentication,
            RetryGuidance::AfterConfigurationChange,
        ),
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
        let request = FetchQualityGateRequest::new(ProjectKey::parse("demo").unwrap());
        let Err(failure) =
            Connector::<FetchQualityGate>::execute(&connector, request, context()).await
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
    let request = DiscoverIssuesRequest::new(ProjectKey::parse("demo").unwrap(), 1).unwrap();
    let Err(failure) = Connector::<DiscoverIssues>::execute(&connector, request, context()).await
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
async fn verify_credentials_requires_a_valid_body_not_just_http_200() {
    let sonarqube = connector(FakeTransport::new([response(200, r#"{"valid":true}"#)]));
    let output = Connector::<VerifyCredentials>::execute(
        &sonarqube,
        VerifyCredentialsRequest::default(),
        context(),
    )
    .await
    .unwrap();
    let seen = sonarqube.transport().seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].url,
        "https://sonarqube.example.test/api/authentication/validate"
    );
    assert!(seen[0].authenticated);
    assert!(output.truth().is_complete());

    let anonymous = connector(FakeTransport::new([response(200, r#"{"valid":false}"#)]));
    let Err(failure) = Connector::<VerifyCredentials>::execute(
        &anonymous,
        VerifyCredentialsRequest::default(),
        context(),
    )
    .await
    else {
        panic!("a not-valid authentication body must fail despite HTTP 200");
    };
    assert_eq!(failure.kind(), FailureKind::Authentication);

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
    let token = "squ_deadbeefdeadbeefdeadbeefdeadbeef";
    let transport = ReqwestSonarTransport::new(Some(token.to_owned())).unwrap();
    let debug = format!("{transport:?}");
    assert!(debug.contains("authenticated"));
    assert!(!debug.contains("squ_"));
    assert!(!debug.contains("deadbeef"));
    for invalid in ["", "with:colon", "tok\nen", &"x".repeat(4097)] {
        assert!(
            ReqwestSonarTransport::new(Some((*invalid).to_owned())).is_err(),
            "must reject {invalid:?}"
        );
    }
    assert!(
        SonarQube::new(
            Url::parse("http://sonarqube.example.test/").unwrap(),
            transport
        )
        .is_err(),
        "a plaintext HTTP base must be rejected"
    );
}
