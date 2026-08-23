use std::{
    collections::VecDeque,
    sync::Mutex,
    time::{Duration, Instant},
};

use super::{
    CancellationToken, Connector, ConnectorDescriptor, FailureKind, InvocationContext, Operation,
    OperationCoordinates, RetryGuidance, Truth, verify_conformance,
};
use crate::{
    CapabilityName, ResourceName,
    aws::{
        ALLOWED_COMMANDS, Aws, AwsCliRunner, CliCommand, CliInvocation, CliOutput, CollectCommand,
        CollectCommandRequest, DiscoverCommands, DiscoverCommandsRequest, MAX_ARG_BYTES,
        MAX_EXTRA_ARGS, MAX_PROFILE_BYTES, MAX_STDERR_BYTES, MAX_STDOUT_BYTES, ProfileName,
        VerifyCredentials, VerifyCredentialsRequest, map_spawn_failure,
    },
};

#[derive(Debug)]
struct SeenInvocation {
    args: Vec<String>,
    timeout: Duration,
    stdout_limit: usize,
    stderr_limit: usize,
}

enum Reply {
    Output(CliOutput),
    Failure(super::ConnectorFailure),
}

struct FakeRunner {
    replies: Mutex<VecDeque<Reply>>,
    seen: Mutex<Vec<SeenInvocation>>,
}

impl FakeRunner {
    fn new(replies: impl IntoIterator<Item = Reply>) -> Self {
        Self {
            replies: Mutex::new(replies.into_iter().collect()),
            seen: Mutex::new(Vec::new()),
        }
    }

    fn seen(&self) -> Vec<SeenInvocation> {
        self.seen
            .lock()
            .expect("seen invocation lock must not be poisoned")
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

impl AwsCliRunner for FakeRunner {
    fn run<'a>(
        &'a self,
        invocation: CliInvocation,
        _context: &'a InvocationContext,
    ) -> super::ConnectorFuture<'a, Result<CliOutput, super::ConnectorFailure>> {
        Box::pin(async move {
            self.seen
                .lock()
                .expect("seen invocation lock must not be poisoned")
                .push(SeenInvocation {
                    args: invocation.args().to_vec(),
                    timeout: invocation.timeout(),
                    stdout_limit: invocation.stdout_limit(),
                    stderr_limit: invocation.stderr_limit(),
                });
            match self
                .replies
                .lock()
                .expect("reply lock must not be poisoned")
                .pop_front()
                .expect("fake runner must have a reply")
            {
                Reply::Output(output) => Ok(output),
                Reply::Failure(failure) => Err(failure),
            }
        })
    }
}

fn output(success: bool, stdout: impl Into<Vec<u8>>) -> Reply {
    Reply::Output(CliOutput::new(success, stdout.into(), false, Vec::new()))
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

fn connector(runner: FakeRunner) -> Aws<FakeRunner> {
    Aws::with_profile_str(None, runner).unwrap()
}

fn collect_request(service: &str, command: &str, args: Vec<String>) -> CollectCommandRequest {
    CollectCommandRequest::new(CliCommand::parse(service, command).unwrap(), args).unwrap()
}

#[test]
fn profile_name_bounds_are_argument_safe() {
    let profile = ProfileName::parse("ops-read.only_1").unwrap();
    assert_eq!(profile.as_str(), "ops-read.only_1");
    let too_long = "a".repeat(MAX_PROFILE_BYTES + 1);
    for invalid in [
        "",
        "-leading-dash",
        "with space",
        "with\nnewline",
        "pro:file",
        "pr\u{f8}file",
        too_long.as_str(),
    ] {
        assert!(
            ProfileName::parse(invalid).is_err(),
            "must reject {invalid:?}"
        );
    }
}

#[test]
fn the_allowlist_rejects_unlisted_and_unsafe_commands_before_any_spawn() {
    assert!(CliCommand::parse("sts", "get-caller-identity").is_ok());
    assert!(CliCommand::parse("ec2", "describe-instances").is_ok());
    // Read-only-looking verbs outside the table stay rejected: prefix
    // heuristics would admit credential egress such as get-login-password.
    for (service, command) in [
        ("ecr", "get-login-password"),
        ("s3", "presign"),
        ("ec2", "terminate-instances"),
        ("s3api", "put-object"),
        ("iam", "create-user"),
        ("sts", "assume-role"),
        ("", ""),
    ] {
        assert!(
            CliCommand::parse(service, command).is_err(),
            "must reject {service} {command}"
        );
    }
}

#[test]
fn extra_arguments_are_bounded_and_forbidden_flags_are_rejected() {
    let command = CliCommand::parse("ec2", "describe-instances").unwrap();
    let valid = vec![
        "--max-items".to_owned(),
        "50".to_owned(),
        "--filters".to_owned(),
        "Name=instance-state-name,Values=running".to_owned(),
        "--region".to_owned(),
        "us-east-1".to_owned(),
        "--query".to_owned(),
        "Reservations[*].Instances[?State.Name=='running']".to_owned(),
    ];
    assert!(CollectCommandRequest::new(command, valid).is_ok());

    for forbidden in [
        "--profile",
        "--profile=other",
        "--output",
        "--output=text",
        "--no-cli-pager",
        "--cli-input-json",
        "--cli-input-yaml",
        "--endpoint-url",
        "--endpoint-url=https://attacker.example.test",
        "--debug",
        "file://payload.json",
        "fileb://payload.bin",
        "--parameters file://payload.json",
        "--OUTPUT",
        "--Profile=dev",
        "FILE://x",
        "FileB://x",
    ] {
        assert!(
            CollectCommandRequest::new(command, vec![forbidden.to_owned()]).is_err(),
            "must reject {forbidden:?}"
        );
    }
    for invalid in ["", "arg\nnewline", "arg\0nul", "arg\ttab"] {
        assert!(
            CollectCommandRequest::new(command, vec![invalid.to_owned()]).is_err(),
            "must reject {invalid:?}"
        );
    }
    assert!(
        CollectCommandRequest::new(command, vec!["x".repeat(MAX_ARG_BYTES + 1)]).is_err(),
        "must reject an oversized argument"
    );
    assert!(
        CollectCommandRequest::new(command, vec!["--max-items".to_owned(); MAX_EXTRA_ARGS + 1])
            .is_err(),
        "must reject too many arguments"
    );
}

#[test]
fn policy_coordinates_are_exact() {
    let request = collect_request("s3api", "list-objects-v2", Vec::new());
    let coordinates = CollectCommand::coordinates(&request);
    assert_eq!(coordinates.capability().as_str(), "commands.inspect");
    assert_eq!(coordinates.resource().as_str(), "aws:s3api.list-objects-v2");
    assert_eq!(
        DiscoverCommands::coordinates(&DiscoverCommandsRequest::default())
            .resource()
            .as_str(),
        "aws:commands"
    );
    // Requests deserialized from stored payloads revalidate the allowlist.
    assert!(
        serde_json::from_str::<CollectCommandRequest>(
            r#"{"command":{"service":"ecr","command":"get-login-password"},"args":[]}"#
        )
        .is_err()
    );
    let roundtrip = serde_json::from_str::<CollectCommandRequest>(
        r#"{"command":{"service":"sts","command":"get-caller-identity"},"args":["--region","eu-west-1"]}"#,
    )
    .unwrap();
    assert_eq!(roundtrip.command().service(), "sts");
    assert_eq!(roundtrip.args(), ["--region", "eu-west-1"]);
}

#[tokio::test]
async fn aws_operations_satisfy_the_connector_conformance_contract() {
    let descriptor = ConnectorDescriptor::new("aws", "cli-1").unwrap();
    let capability = CapabilityName::parse("commands.inspect").unwrap();

    verify_conformance::<_, VerifyCredentials>(
        &connector(FakeRunner::new([])),
        VerifyCredentialsRequest::default(),
        &descriptor,
        &OperationCoordinates::new(
            CapabilityName::parse("connection.verify").unwrap(),
            ResourceName::parse("aws:api").unwrap(),
        ),
    )
    .await
    .unwrap();
    verify_conformance::<_, DiscoverCommands>(
        &connector(FakeRunner::new([])),
        DiscoverCommandsRequest::default(),
        &descriptor,
        &OperationCoordinates::new(
            capability.clone(),
            ResourceName::parse("aws:commands").unwrap(),
        ),
    )
    .await
    .unwrap();
    verify_conformance::<_, CollectCommand>(
        &connector(FakeRunner::new([])),
        collect_request("rds", "describe-db-instances", Vec::new()),
        &descriptor,
        &OperationCoordinates::new(
            capability,
            ResourceName::parse("aws:rds.describe-db-instances").unwrap(),
        ),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn verify_credentials_requires_a_named_account_and_arn_not_just_exit_zero() {
    let body = r#"{"UserId":"AIDAEXAMPLE","Account":"123456789012","Arn":"arn:aws:iam::123456789012:user/ops"}"#;
    let aws = Aws::with_profile_str(Some("ops"), FakeRunner::new([output(true, body)])).unwrap();
    let result = Connector::<VerifyCredentials>::execute(
        &aws,
        VerifyCredentialsRequest::default(),
        context(),
    )
    .await
    .unwrap();
    assert!(result.truth().is_complete());
    let seen = aws.runner().seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].args,
        [
            "sts",
            "get-caller-identity",
            "--profile",
            "ops",
            "--output",
            "json",
            "--no-cli-pager"
        ]
    );
    assert_eq!(seen[0].timeout, Duration::from_secs(30));
    assert_eq!(seen[0].stdout_limit, MAX_STDOUT_BYTES);
    assert_eq!(seen[0].stderr_limit, MAX_STDERR_BYTES);
    assert!(aws.runner().is_empty());

    let denied = Reply::Output(CliOutput::new(
        false,
        Vec::new(),
        false,
        b"Unable to locate credentials. You can configure credentials by running \"aws configure\"."
            .to_vec(),
    ));
    let anonymous = connector(FakeRunner::new([denied]));
    let Err(failure) = Connector::<VerifyCredentials>::execute(
        &anonymous,
        VerifyCredentialsRequest::default(),
        context(),
    )
    .await
    else {
        panic!("a failing caller-identity probe must fail");
    };
    assert_eq!(failure.kind(), FailureKind::Authentication);
    assert!(failure.message().as_str().contains("Unable to locate"));

    for malformed in [r"not json", r#"{"Account":"","Arn":""}"#, r#"{"Other":1}"#] {
        let aws = connector(FakeRunner::new([output(true, malformed)]));
        let Err(failure) = Connector::<VerifyCredentials>::execute(
            &aws,
            VerifyCredentialsRequest::default(),
            context(),
        )
        .await
        else {
            panic!("malformed identity JSON must fail: {malformed:?}");
        };
        assert_eq!(failure.kind(), FailureKind::Remote);
        assert_eq!(failure.retry_guidance(), RetryGuidance::Never);
    }
}

#[tokio::test]
async fn command_discovery_returns_the_full_allowlist_without_spawning() {
    let aws = connector(FakeRunner::new([]));
    let result =
        Connector::<DiscoverCommands>::execute(&aws, DiscoverCommandsRequest::default(), context())
            .await
            .unwrap();
    assert!(result.truth().is_complete());
    let listed = result
        .value()
        .commands()
        .iter()
        .map(|command| (command.service(), command.command()))
        .collect::<Vec<_>>();
    assert_eq!(listed, ALLOWED_COMMANDS);
    assert!(aws.runner().seen().is_empty());
}

#[tokio::test]
async fn command_collection_returns_stdout_and_is_partial_when_truncated() {
    let body = r#"{"Reservations":[]}"#;
    let aws = connector(FakeRunner::new([output(true, body)]));
    let request = collect_request(
        "ec2",
        "describe-instances",
        vec!["--max-items".to_owned(), "50".to_owned()],
    );
    let result = Connector::<CollectCommand>::execute(&aws, request, context())
        .await
        .unwrap();
    assert!(result.truth().is_complete());
    assert_eq!(result.value().service(), "ec2");
    assert_eq!(result.value().command(), "describe-instances");
    assert_eq!(result.value().output_json(), body);
    let seen = aws.runner().seen();
    assert_eq!(
        seen[0].args,
        [
            "ec2",
            "describe-instances",
            "--max-items",
            "50",
            "--output",
            "json",
            "--no-cli-pager"
        ]
    );
    assert!(aws.runner().is_empty());

    let truncated = Reply::Output(CliOutput::new(
        true,
        b"{\"partial\":".to_vec(),
        true,
        Vec::new(),
    ));
    let capped = connector(FakeRunner::new([truncated]));
    let request = collect_request("s3api", "list-buckets", Vec::new());
    let result = Connector::<CollectCommand>::execute(&capped, request, context())
        .await
        .unwrap();
    let Truth::Partial { reason } = result.truth() else {
        panic!("capped stdout must be partial")
    };
    assert!(reason.as_str().contains("truncated"));

    let failing = Reply::Output(CliOutput::new(
        false,
        Vec::new(),
        false,
        b"An error occurred (AccessDenied) when calling the ListBuckets operation".to_vec(),
    ));
    let denied = connector(FakeRunner::new([failing]));
    let request = collect_request("s3api", "list-buckets", Vec::new());
    let Err(failure) = Connector::<CollectCommand>::execute(&denied, request, context()).await
    else {
        panic!("a nonzero CLI exit must fail");
    };
    assert_eq!(failure.kind(), FailureKind::Remote);
    assert_eq!(failure.retry_guidance(), RetryGuidance::Never);
    assert!(failure.message().as_str().contains("AccessDenied"));
}

#[tokio::test]
async fn runner_failures_pass_through_typed() {
    let aws = connector(FakeRunner::new([Reply::Failure(
        super::ConnectorFailure::timeout(),
    )]));
    let request = collect_request("logs", "describe-log-groups", Vec::new());
    let Err(failure) = Connector::<CollectCommand>::execute(&aws, request, context()).await else {
        panic!("a runner timeout must fail the operation");
    };
    assert_eq!(failure.kind(), FailureKind::Timeout);
}

#[test]
fn a_missing_aws_binary_maps_to_a_clear_not_found_failure() {
    let failure = map_spawn_failure(&std::io::Error::from(std::io::ErrorKind::NotFound));
    assert_eq!(failure.kind(), FailureKind::NotFound);
    assert_eq!(
        failure.message().as_str(),
        "the aws CLI was not found on PATH"
    );
    let failure = map_spawn_failure(&std::io::Error::from(std::io::ErrorKind::PermissionDenied));
    assert_eq!(failure.kind(), FailureKind::Remote);
}

#[test]
fn connector_construction_validates_the_optional_profile() {
    assert!(Aws::with_profile_str(Some("ops"), FakeRunner::new([])).is_ok());
    assert!(Aws::with_profile_str(None, FakeRunner::new([])).is_ok());
    for invalid in ["", "-leading", "with space"] {
        assert!(
            Aws::with_profile_str(Some(invalid), FakeRunner::new([])).is_err(),
            "must reject {invalid:?}"
        );
    }
}
