use std::{
    collections::VecDeque,
    sync::Mutex,
    time::{Duration, Instant},
};

use url::Url;

use super::{
    CancellationToken, Connector, ConnectorDescriptor, FailureKind, InvocationContext, Operation,
    OperationCoordinates, RetryGuidance, verify_conformance,
};
use crate::{
    CapabilityName, ResourceName,
    jenkins::{
        BuildNumber, CollectConsoleLog, CollectConsoleLogRequest, DiscoverBuilds,
        DiscoverBuildsRequest, DiscoverJobs, DiscoverJobsRequest, Jenkins, JenkinsTransport,
        JobPath, MAX_CONSOLE_LOG_BYTES, MAX_DISCOVERED_BUILDS, MAX_DISCOVERED_JOBS,
        MAX_JOB_SEGMENTS, ReqwestJenkinsTransport, TransportRequest, TransportResponse,
        VerifyCredentials, VerifyCredentialsRequest, console_artifact_name,
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

impl JenkinsTransport for FakeTransport {
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

fn connector(transport: FakeTransport) -> Jenkins<FakeTransport> {
    Jenkins::new(
        Url::parse("https://jenkins.example.test/").unwrap(),
        transport,
    )
    .unwrap()
}

fn job_json(name: &str, color: &str) -> String {
    format!(
        r#"{{"name":"{name}","url":"https://jenkins.example.test/job/{name}/","color":"{color}"}}"#
    )
}

fn build_json(number: u64, result: &str) -> String {
    format!(
        r#"{{"number":{number},"result":"{result}","timestamp":1700000000000,"duration":120000,"url":"https://jenkins.example.test/job/folder/job/app/{number}/"}}"#
    )
}

#[test]
fn job_path_bounds_and_policy_coordinates_are_exact() {
    let job = JobPath::parse("folder/app").unwrap();
    assert_eq!(job.as_coordinate(), "folder/app");
    assert_eq!(serde_json::to_string(&job).unwrap(), r#""folder/app""#);
    assert_eq!(
        serde_json::from_str::<JobPath>(r#""folder/app""#).unwrap(),
        job
    );
    let too_deep = ["a"; MAX_JOB_SEGMENTS + 1].join("/");
    for invalid in ["", "a//b", "../job", "a/..", "job name", too_deep.as_str()] {
        assert!(JobPath::parse(invalid).is_err(), "must reject {invalid:?}");
    }
    assert!(BuildNumber::new(0).is_err());
    assert!(DiscoverJobsRequest::new(0).is_err());
    assert!(DiscoverJobsRequest::new(MAX_DISCOVERED_JOBS + 1).is_err());
    assert!(DiscoverBuildsRequest::new(job.clone(), MAX_DISCOVERED_BUILDS + 1).is_err());
    assert!(
        CollectConsoleLogRequest::new(
            job.clone(),
            BuildNumber::new(8).unwrap(),
            MAX_CONSOLE_LOG_BYTES + 1,
        )
        .is_err()
    );

    let discovery = DiscoverJobsRequest::new(5).unwrap();
    let coordinates = DiscoverJobs::coordinates(&discovery);
    assert_eq!(coordinates.capability().as_str(), "jobs.inspect");
    assert_eq!(coordinates.resource().as_str(), "jenkins:server");
    let builds = DiscoverBuildsRequest::new(job.clone(), 5).unwrap();
    assert_eq!(
        DiscoverBuilds::coordinates(&builds).resource().as_str(),
        "jenkins:folder/app"
    );
    let log =
        CollectConsoleLogRequest::new(job.clone(), BuildNumber::new(8).unwrap(), 1024).unwrap();
    assert_eq!(
        CollectConsoleLog::coordinates(&log).resource().as_str(),
        "jenkins:folder/app/builds/8"
    );
    assert_eq!(
        console_artifact_name(&job, BuildNumber::new(8).unwrap()),
        "jenkins-folder-app-build-8.log"
    );
}

#[tokio::test]
async fn jenkins_operations_satisfy_the_connector_conformance_contract() {
    let descriptor = ConnectorDescriptor::new("jenkins", "json-api-1").unwrap();
    let capability = CapabilityName::parse("jobs.inspect").unwrap();
    let job = JobPath::parse("folder/app").unwrap();

    verify_conformance::<_, DiscoverJobs>(
        &connector(FakeTransport::new([])),
        DiscoverJobsRequest::new(5).unwrap(),
        &descriptor,
        &OperationCoordinates::new(
            capability.clone(),
            ResourceName::parse("jenkins:server").unwrap(),
        ),
    )
    .await
    .unwrap();
    verify_conformance::<_, DiscoverBuilds>(
        &connector(FakeTransport::new([])),
        DiscoverBuildsRequest::new(job.clone(), 5).unwrap(),
        &descriptor,
        &OperationCoordinates::new(
            capability.clone(),
            ResourceName::parse("jenkins:folder/app").unwrap(),
        ),
    )
    .await
    .unwrap();
    verify_conformance::<_, CollectConsoleLog>(
        &connector(FakeTransport::new([])),
        CollectConsoleLogRequest::new(job, BuildNumber::new(8).unwrap(), 1024).unwrap(),
        &descriptor,
        &OperationCoordinates::new(
            capability,
            ResourceName::parse("jenkins:folder/app/builds/8").unwrap(),
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
            ResourceName::parse("jenkins:api").unwrap(),
        ),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn job_discovery_is_bounded_sorted_and_authenticated() {
    let body = format!(
        r#"{{"jobs":[{},{}]}}"#,
        job_json("zeta", "red"),
        job_json("alpha", "blue")
    );
    let connector = connector(FakeTransport::new([response(200, body)]));
    let request = DiscoverJobsRequest::new(2).unwrap();
    let output = Connector::<DiscoverJobs>::execute(&connector, request, context())
        .await
        .unwrap();
    assert!(output.truth().is_complete());
    assert_eq!(
        output
            .value()
            .jobs()
            .iter()
            .map(crate::jenkins::JenkinsJob::name)
            .collect::<Vec<_>>(),
        vec!["alpha", "zeta"]
    );
    let seen = connector.transport().seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].url,
        "https://jenkins.example.test/api/json?tree=jobs[name,url,color]{0,2}"
    );
    assert!(seen[0].authenticated);
    assert_eq!(seen[0].response_limit, 1024 * 1024);
    assert!(connector.transport().is_empty());

    let overflowing = format!(
        r#"{{"jobs":[{},{}]}}"#,
        job_json("one", "red"),
        job_json("two", "red")
    );
    let strict = self::connector(FakeTransport::new([response(200, overflowing)]));
    let result = Connector::<DiscoverJobs>::execute(
        &strict,
        DiscoverJobsRequest::new(1).unwrap(),
        context(),
    )
    .await;
    let Err(failure) = result else {
        panic!("an over-limit job listing must fail")
    };
    assert_eq!(failure.kind(), FailureKind::Remote);
}

#[tokio::test]
async fn folder_job_paths_map_to_nested_job_url_segments() {
    let body = format!(
        r#"{{"builds":[{},{}]}}"#,
        build_json(8, "FAILURE"),
        build_json(3, "SUCCESS")
    );
    let connector = connector(FakeTransport::new([response(200, body)]));
    let request = DiscoverBuildsRequest::new(JobPath::parse("folder/app").unwrap(), 2).unwrap();
    let output = Connector::<DiscoverBuilds>::execute(&connector, request, context())
        .await
        .unwrap();
    assert_eq!(output.value().job(), "folder/app");
    assert_eq!(
        output
            .value()
            .builds()
            .iter()
            .map(|build| build.number().get())
            .collect::<Vec<_>>(),
        vec![3, 8]
    );
    assert_eq!(output.value().builds()[1].result(), Some("FAILURE"));
    let seen = connector.transport().seen();
    assert_eq!(
        seen[0].url,
        "https://jenkins.example.test/job/folder/job/app/api/json?tree=builds[number,result,timestamp,duration,url]{0,2}"
    );
    assert!(seen[0].authenticated);
    assert!(connector.transport().is_empty());
}

#[tokio::test]
async fn console_log_collection_returns_the_exact_artifact() {
    let connector = connector(FakeTransport::new([response(
        200,
        b"Started by user\nFinished: FAILURE\n".to_vec(),
    )]));
    let request = CollectConsoleLogRequest::new(
        JobPath::parse("folder/app").unwrap(),
        BuildNumber::new(8).unwrap(),
        1024,
    )
    .unwrap();
    let output = Connector::<CollectConsoleLog>::execute(&connector, request, context())
        .await
        .unwrap();
    assert!(output.truth().is_complete());
    assert_eq!(output.value().job(), "folder/app");
    assert_eq!(output.value().build().get(), 8);
    assert_eq!(
        output.value().artifact_name(),
        "jenkins-folder-app-build-8.log"
    );
    assert_eq!(output.value().byte_len(), 34);
    assert_eq!(output.artifacts().len(), 1);
    assert_eq!(
        output.artifacts()[0].bytes(),
        b"Started by user\nFinished: FAILURE\n"
    );
    let seen = connector.transport().seen();
    assert_eq!(
        seen[0].url,
        "https://jenkins.example.test/job/folder/job/app/8/consoleText"
    );
    assert!(seen[0].authenticated);
    assert_eq!(seen[0].response_limit, 1024);
    assert!(connector.transport().is_empty());
}

#[tokio::test]
async fn oversized_console_logs_are_rejected_at_the_byte_cap() {
    let connector = connector(FakeTransport::new([Reply::Failure(
        super::ConnectorFailure::response_too_large(1024),
    )]));
    let request = CollectConsoleLogRequest::new(
        JobPath::parse("app").unwrap(),
        BuildNumber::new(8).unwrap(),
        1024,
    )
    .unwrap();
    let Err(failure) =
        Connector::<CollectConsoleLog>::execute(&connector, request, context()).await
    else {
        panic!("an oversized console log must fail")
    };
    assert_eq!(failure.kind(), FailureKind::ResponseTooLarge);
    assert_eq!(failure.response_limit(), Some(1024));
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
        let request = DiscoverBuildsRequest::new(JobPath::parse("app").unwrap(), 1).unwrap();
        let Err(failure) =
            Connector::<DiscoverBuilds>::execute(&connector, request, context()).await
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
    let request = DiscoverJobsRequest::new(1).unwrap();
    let Err(failure) = Connector::<DiscoverJobs>::execute(&connector, request, context()).await
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
async fn verify_credentials_probes_the_user_identity_and_reports_no_remote_details() {
    let jenkins = connector(FakeTransport::new([response(
        200,
        r#"{"id":"probe","fullName":"Probe User"}"#,
    )]));

    let output = Connector::<VerifyCredentials>::execute(
        &jenkins,
        VerifyCredentialsRequest::default(),
        context(),
    )
    .await
    .unwrap();

    let seen = jenkins.transport().seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].url, "https://jenkins.example.test/me/api/json");
    assert!(seen[0].authenticated);
    assert!(output.truth().is_complete());
    let rendered = format!("{} {:?}", output.summary(), output.value());
    assert!(!rendered.contains("probe"), "remote identity must not leak");

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
    let secret = "ci-bot:11deadbeefdeadbeefdeadbeefdeadbeef";
    let transport = ReqwestJenkinsTransport::new(Some(secret.to_owned())).unwrap();
    let debug = format!("{transport:?}");
    assert!(debug.contains("authenticated"));
    assert!(!debug.contains("ci-bot"));
    assert!(!debug.contains("deadbeef"));
    for invalid in ["", "no-separator", ":token", "user:", "user:tok\nen"] {
        assert!(
            ReqwestJenkinsTransport::new(Some(invalid.to_owned())).is_err(),
            "must reject {invalid:?}"
        );
    }
    assert!(
        Jenkins::new(
            Url::parse("http://jenkins.example.test/").unwrap(),
            transport
        )
        .is_err(),
        "a plaintext HTTP base must be rejected"
    );
}
