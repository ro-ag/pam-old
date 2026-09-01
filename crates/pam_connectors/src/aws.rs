//! Typed AWS CLI passthrough operations.
//!
//! Pam stores no AWS keys: the connector spawns the operator's own `aws`
//! binary directly (never through a shell) with an allowlisted read-only
//! command, daemon-controlled output flags, and an optional `--profile` name
//! as its only stored configuration.

use std::{error::Error, fmt, io::ErrorKind, process::Stdio, sync::Arc, time::Duration};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{
    BoundedSummary, CapabilityName, Connector, ConnectorDescriptor, ConnectorFailure,
    ConnectorFuture, ConnectorOutput, FailureMessage, InvocationContext, Operation,
    OperationCoordinates, OperationEffect, ResourceName, Truth,
};

pub const MAX_STDOUT_BYTES: usize = 256 * 1024;
pub const MAX_STDERR_BYTES: usize = 4 * 1024;
pub const MAX_EXTRA_ARGS: usize = 32;
pub const MAX_ARG_BYTES: usize = 512;
pub const MAX_PROFILE_BYTES: usize = 64;

const MAX_STDERR_EXCERPT_BYTES: usize = 512;
const CLI_TIMEOUT: Duration = Duration::from_secs(30);

/// The exact read-only `(service, command)` pairs this connector may execute.
///
/// The table is deliberately exhaustive rather than a verb-prefix heuristic:
/// prefix matching over `get`/`list`/`describe` would still admit commands
/// with side effects or secret egress, such as `ecr get-login-password`,
/// `s3 presign`, and calls that write local files through `outfile`
/// parameters. Every addition must be reviewed as an exact pair.
pub const ALLOWED_COMMANDS: &[(&str, &str)] = &[
    ("sts", "get-caller-identity"),
    ("ec2", "describe-instances"),
    ("ec2", "describe-security-groups"),
    ("ec2", "describe-vpcs"),
    ("ec2", "describe-subnets"),
    ("s3api", "list-buckets"),
    ("s3api", "list-objects-v2"),
    ("s3api", "get-bucket-location"),
    ("iam", "list-users"),
    ("iam", "list-roles"),
    ("iam", "get-user"),
    ("iam", "list-attached-role-policies"),
    ("cloudformation", "list-stacks"),
    ("cloudformation", "describe-stacks"),
    ("cloudformation", "describe-stack-events"),
    ("lambda", "list-functions"),
    ("lambda", "get-function-configuration"),
    ("logs", "describe-log-groups"),
    ("logs", "describe-log-streams"),
    ("logs", "filter-log-events"),
    ("ecs", "list-clusters"),
    ("ecs", "list-services"),
    ("ecs", "describe-services"),
    ("rds", "describe-db-instances"),
    ("cloudwatch", "describe-alarms"),
    ("cloudwatch", "get-metric-data"),
];

/// Flags the daemon controls or refuses outright; request arguments may never
/// carry them, including in `--flag=value` form.
const FORBIDDEN_FLAGS: [&str; 7] = [
    "--profile",
    "--output",
    "--no-cli-pager",
    "--cli-input-json",
    "--cli-input-yaml",
    "--endpoint-url",
    "--debug",
];

/// A validated AWS CLI profile name.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct ProfileName {
    name: String,
}

impl ProfileName {
    /// Parses a bounded AWS CLI profile name.
    ///
    /// # Errors
    ///
    /// Returns an error unless the name is one to 64 bytes of ASCII
    /// alphanumerics, `_`, `.`, or `-`, not starting with `-` (the value
    /// becomes a process argument).
    pub fn parse(value: impl AsRef<str>) -> Result<Self, InvalidProfileName> {
        let name = value.as_ref();
        if name.is_empty()
            || name.len() > MAX_PROFILE_BYTES
            || name.starts_with('-')
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
        {
            return Err(InvalidProfileName);
        }
        Ok(Self {
            name: name.to_owned(),
        })
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.name
    }
}

impl fmt::Debug for ProfileName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.name)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidProfileName;

impl fmt::Display for InvalidProfileName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(
            "AWS profile name must be one to 64 ASCII alphanumerics, `_`, `.`, or `-`, not \
             starting with `-`",
        )
    }
}

impl Error for InvalidProfileName {}

/// One allowlisted `(service, command)` pair; unlisted pairs cannot exist.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct CliCommand {
    service: &'static str,
    command: &'static str,
}

impl CliCommand {
    /// Resolves a `(service, command)` pair against [`ALLOWED_COMMANDS`].
    ///
    /// # Errors
    ///
    /// Returns an error for any pair not in the read-only allowlist.
    pub fn parse(service: &str, command: &str) -> Result<Self, CommandNotAllowed> {
        ALLOWED_COMMANDS
            .iter()
            .find(|(allowed_service, allowed_command)| {
                *allowed_service == service && *allowed_command == command
            })
            .map(|&(service, command)| Self { service, command })
            .ok_or(CommandNotAllowed)
    }

    #[must_use]
    pub const fn service(&self) -> &'static str {
        self.service
    }

    #[must_use]
    pub const fn command(&self) -> &'static str {
        self.command
    }

    fn resource(self) -> ResourceName {
        ResourceName::parse(format!("aws:{}.{}", self.service, self.command))
            .expect("allowlisted AWS command coordinates fit the policy resource bound")
    }
}

impl fmt::Debug for CliCommand {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} {}", self.service, self.command)
    }
}

impl<'de> Deserialize<'de> for CliCommand {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawCliCommand {
            service: String,
            command: String,
        }
        let raw = RawCliCommand::deserialize(deserializer)?;
        Self::parse(&raw.service, &raw.command).map_err(serde::de::Error::custom)
    }
}

impl Serialize for CliCommand {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        struct RawCliCommand<'a> {
            service: &'a str,
            command: &'a str,
        }
        RawCliCommand {
            service: self.service,
            command: self.command,
        }
        .serialize(serializer)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CommandNotAllowed;

impl fmt::Display for CommandNotAllowed {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AWS CLI command is not in the read-only allowlist")
    }
}

impl Error for CommandNotAllowed {}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CollectCommandRequest {
    command: CliCommand,
    args: Vec<String>,
}

impl CollectCommandRequest {
    /// # Errors
    ///
    /// Returns an error unless every extra argument is within the count,
    /// length, character-set, and forbidden-flag bounds.
    pub fn new(command: CliCommand, args: Vec<String>) -> Result<Self, InvalidCliArguments> {
        validate_extra_args(&args)?;
        Ok(Self { command, args })
    }

    #[must_use]
    pub const fn command(&self) -> &CliCommand {
        &self.command
    }

    #[must_use]
    pub fn args(&self) -> &[String] {
        &self.args
    }

    fn validate(&self) -> Result<(), ConnectorFailure> {
        validate_extra_args(&self.args)
            .map_err(|_| invalid_request("AWS CLI extra arguments are invalid"))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidCliArguments;

impl fmt::Display for InvalidCliArguments {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AWS CLI extra arguments exceed the connector SDK bounds")
    }
}

impl Error for InvalidCliArguments {}

fn validate_extra_args(args: &[String]) -> Result<(), InvalidCliArguments> {
    if args.len() > MAX_EXTRA_ARGS {
        return Err(InvalidCliArguments);
    }
    for arg in args {
        if arg.is_empty() || arg.len() > MAX_ARG_BYTES || !arg.bytes().all(valid_arg_byte) {
            return Err(InvalidCliArguments);
        }
        // `file://` and `fileb://` make the CLI read local files into request
        // parameters, so they are refused anywhere inside an argument.
        let lowered = arg.to_ascii_lowercase();
        if lowered.contains("file://") || lowered.contains("fileb://") {
            return Err(InvalidCliArguments);
        }
        if FORBIDDEN_FLAGS
            .iter()
            .any(|flag| lowered == *flag || lowered.starts_with(&format!("{flag}=")))
        {
            return Err(InvalidCliArguments);
        }
    }
    Ok(())
}

/// Conservative argument character set: printable ASCII needed for filters,
/// `JMESPath` queries, and ARNs — never NUL, newlines, or other controls.
fn valid_arg_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'@' | b'_'
                | b','
                | b':'
                | b'='
                | b'+'
                | b'/'
                | b'.'
                | b'*'
                | b'?'
                | b'['
                | b']'
                | b'{'
                | b'}'
                | b'"'
                | b'\''
                | b'%'
                | b' '
                | b'-'
        )
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CollectCommandResponse {
    service: String,
    command: String,
    output_json: String,
}

impl CollectCommandResponse {
    #[must_use]
    pub fn service(&self) -> &str {
        &self.service
    }

    #[must_use]
    pub fn command(&self) -> &str {
        &self.command
    }

    #[must_use]
    pub fn output_json(&self) -> &str {
        &self.output_json
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct DiscoverCommandsRequest {}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DiscoverCommandsResponse {
    commands: Vec<CliCommand>,
}

impl DiscoverCommandsResponse {
    #[must_use]
    pub fn commands(&self) -> &[CliCommand] {
        &self.commands
    }
}

pub struct DiscoverCommands;

impl Operation for DiscoverCommands {
    type Request = DiscoverCommandsRequest;
    type Response = DiscoverCommandsResponse;

    const EFFECT: OperationEffect = OperationEffect::ReadOnly;

    fn coordinates(_request: &Self::Request) -> OperationCoordinates {
        OperationCoordinates::new(
            capability(),
            ResourceName::parse("aws:commands").expect("static AWS resource is valid"),
        )
    }
}

pub struct CollectCommand;

impl Operation for CollectCommand {
    type Request = CollectCommandRequest;
    type Response = CollectCommandResponse;

    const EFFECT: OperationEffect = OperationEffect::ReadOnly;

    fn coordinates(request: &Self::Request) -> OperationCoordinates {
        OperationCoordinates::new(capability(), request.command.resource())
    }
}

/// Minimal read-only probe used by connector self-tests.
///
/// It verifies the local `aws` binary and the effective credential chain by
/// asking STS for the caller identity; no resource data is read and no remote
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
            CapabilityName::parse("connection.verify").expect("static AWS capability is valid"),
            ResourceName::parse("aws:api").expect("static AWS resource is valid"),
        )
    }
}

/// AWS CLI connector over an injected bounded subprocess runner.
pub struct Aws<R> {
    profile: Option<ProfileName>,
    runner: R,
}

impl<R> Aws<R> {
    #[must_use]
    pub const fn new(profile: Option<ProfileName>, runner: R) -> Self {
        Self { profile, runner }
    }

    /// Parses and validates an optional textual profile name for callers
    /// without a [`ProfileName`].
    ///
    /// # Errors
    ///
    /// Returns an error unless the profile name, when present, is a bounded
    /// argument-safe value.
    pub fn with_profile_str(profile: Option<&str>, runner: R) -> Result<Self, InvalidProfileName> {
        let profile = profile.map(ProfileName::parse).transpose()?;
        Ok(Self::new(profile, runner))
    }

    #[must_use]
    pub const fn runner(&self) -> &R {
        &self.runner
    }

    fn invocation(&self, command: CliCommand, extra_args: &[String]) -> CliInvocation {
        let mut args = vec![command.service.to_owned(), command.command.to_owned()];
        args.extend(extra_args.iter().cloned());
        if let Some(profile) = &self.profile {
            args.push("--profile".to_owned());
            args.push(profile.as_str().to_owned());
        }
        // The output format and pager flags are daemon-controlled and appended
        // last so request arguments can never override them.
        args.push("--output".to_owned());
        args.push("json".to_owned());
        args.push("--no-cli-pager".to_owned());
        CliInvocation {
            args,
            timeout: CLI_TIMEOUT,
            stdout_limit: MAX_STDOUT_BYTES,
            stderr_limit: MAX_STDERR_BYTES,
        }
    }
}

impl<R: AwsCliRunner> Connector<VerifyCredentials> for Aws<R> {
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
            let identity = CliCommand::parse("sts", "get-caller-identity")
                .expect("the caller-identity probe is allowlisted");
            let output = self
                .runner
                .run(self.invocation(identity, &[]), &context)
                .await?;
            if !output.success {
                return Err(ConnectorFailure::authentication(cli_failure_message(
                    "AWS rejected the caller-identity probe",
                    &output.stderr,
                )));
            }
            // Exit status zero alone is not acceptance: the body must be
            // caller-identity JSON naming a nonempty account and ARN.
            let identified = serde_json::from_slice::<CallerIdentityEnvelope>(&output.stdout)
                .is_ok_and(|identity| !identity.account.is_empty() && !identity.arn.is_empty());
            if !identified {
                return Err(remote_failure(
                    "AWS returned malformed caller-identity JSON",
                ));
            }
            ConnectorOutput::new(
                VerifyCredentialsResponse {},
                summary("AWS caller identity verified via the aws CLI".to_owned())?,
                Truth::Complete,
                Vec::new(),
                Vec::new(),
            )
            .map_err(|_| remote_failure("AWS verification output exceeded SDK bounds"))
        })
    }
}

impl<R: AwsCliRunner> Connector<DiscoverCommands> for Aws<R> {
    fn descriptor(&self) -> ConnectorDescriptor {
        descriptor()
    }

    fn execute(
        &self,
        _request: DiscoverCommandsRequest,
        context: InvocationContext,
    ) -> ConnectorFuture<'_, crate::ConnectorResult<DiscoverCommandsResponse>> {
        Box::pin(async move {
            context.preflight(OperationEffect::ReadOnly)?;
            // Discovery is the static allowlist itself; no subprocess runs.
            let commands = ALLOWED_COMMANDS
                .iter()
                .map(|&(service, command)| CliCommand { service, command })
                .collect::<Vec<_>>();
            let count = commands.len();
            ConnectorOutput::new(
                DiscoverCommandsResponse { commands },
                summary(format!("{count} read-only AWS CLI commands are executable"))?,
                Truth::Complete,
                Vec::new(),
                Vec::new(),
            )
            .map_err(|_| remote_failure("AWS command discovery output exceeded SDK bounds"))
        })
    }
}

impl<R: AwsCliRunner> Connector<CollectCommand> for Aws<R> {
    fn descriptor(&self) -> ConnectorDescriptor {
        descriptor()
    }

    fn execute(
        &self,
        request: CollectCommandRequest,
        context: InvocationContext,
    ) -> ConnectorFuture<'_, crate::ConnectorResult<CollectCommandResponse>> {
        Box::pin(async move {
            context.preflight(OperationEffect::ReadOnly)?;
            request.validate()?;
            let command = request.command;
            let output = self
                .runner
                .run(self.invocation(command, &request.args), &context)
                .await?;
            if !output.success {
                return Err(ConnectorFailure::remote(
                    cli_failure_message("AWS CLI command exited unsuccessfully", &output.stderr),
                    false,
                ));
            }
            let truth = if output.stdout_truncated {
                Truth::Partial {
                    reason: summary(format!(
                        "stdout was truncated at the {MAX_STDOUT_BYTES}-byte cap"
                    ))?,
                }
            } else {
                Truth::Complete
            };
            ConnectorOutput::new(
                CollectCommandResponse {
                    service: command.service.to_owned(),
                    command: command.command.to_owned(),
                    // Lossy conversion also repairs a cap cut mid-character.
                    output_json: String::from_utf8_lossy(&output.stdout).into_owned(),
                },
                summary(format!(
                    "aws {} {} completed",
                    command.service, command.command
                ))?,
                truth,
                Vec::new(),
                Vec::new(),
            )
            .map_err(|_| remote_failure("AWS command output exceeded SDK bounds"))
        })
    }
}

#[derive(Deserialize)]
struct CallerIdentityEnvelope {
    #[serde(rename = "Account")]
    account: String,
    #[serde(rename = "Arn")]
    arn: String,
}

/// One fully daemon-controlled CLI invocation kept crate-visible for
/// deterministic connector tests.
pub struct CliInvocation {
    args: Vec<String>,
    timeout: Duration,
    stdout_limit: usize,
    stderr_limit: usize,
}

impl CliInvocation {
    #[must_use]
    pub fn args(&self) -> &[String] {
        &self.args
    }

    #[must_use]
    pub const fn timeout(&self) -> Duration {
        self.timeout
    }

    #[must_use]
    pub const fn stdout_limit(&self) -> usize {
        self.stdout_limit
    }

    #[must_use]
    pub const fn stderr_limit(&self) -> usize {
        self.stderr_limit
    }
}

/// Bounded CLI output kept crate-visible for deterministic connector tests.
pub struct CliOutput {
    success: bool,
    stdout: Vec<u8>,
    stdout_truncated: bool,
    stderr: Vec<u8>,
}

impl CliOutput {
    #[must_use]
    pub const fn new(
        success: bool,
        stdout: Vec<u8>,
        stdout_truncated: bool,
        stderr: Vec<u8>,
    ) -> Self {
        Self {
            success,
            stdout,
            stdout_truncated,
            stderr,
        }
    }
}

pub trait AwsCliRunner: Send + Sync {
    fn run<'a>(
        &'a self,
        invocation: CliInvocation,
        context: &'a InvocationContext,
    ) -> ConnectorFuture<'a, Result<CliOutput, ConnectorFailure>>;
}

impl<R: AwsCliRunner + ?Sized> AwsCliRunner for Arc<R> {
    fn run<'a>(
        &'a self,
        invocation: CliInvocation,
        context: &'a InvocationContext,
    ) -> ConnectorFuture<'a, Result<CliOutput, ConnectorFailure>> {
        (**self).run(invocation, context)
    }
}

/// Production runner spawning the local `aws` binary directly — no shell —
/// with a hard timeout, kill-on-drop, and bounded output reads.
#[derive(Clone, Copy, Debug, Default)]
pub struct TokioAwsCliRunner;

impl TokioAwsCliRunner {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl AwsCliRunner for TokioAwsCliRunner {
    fn run<'a>(
        &'a self,
        invocation: CliInvocation,
        context: &'a InvocationContext,
    ) -> ConnectorFuture<'a, Result<CliOutput, ConnectorFailure>> {
        Box::pin(async move {
            context.preflight(OperationEffect::ReadOnly)?;
            let remaining = context.remaining().ok_or_else(ConnectorFailure::timeout)?;
            let mut child = tokio::process::Command::new("aws")
                .args(invocation.args())
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .map_err(|error| map_spawn_failure(&error))?;
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| remote_failure("AWS CLI stdout could not be captured"))?;
            let stderr = child
                .stderr
                .take()
                .ok_or_else(|| remote_failure("AWS CLI stderr could not be captured"))?;
            let stdout_task = tokio::spawn(read_bounded(stdout, invocation.stdout_limit()));
            let stderr_task = tokio::spawn(read_bounded(stderr, invocation.stderr_limit()));
            let deadline = remaining.min(invocation.timeout());
            let status = match tokio::time::timeout(deadline, child.wait()).await {
                Ok(Ok(status)) => status,
                Ok(Err(_)) => {
                    return Err(remote_failure("AWS CLI process could not be awaited"));
                }
                Err(_) => {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    return Err(ConnectorFailure::timeout());
                }
            };
            let (stdout, stdout_truncated) = stdout_task
                .await
                .ok()
                .and_then(Result::ok)
                .ok_or_else(|| remote_failure("AWS CLI stdout could not be read"))?;
            let (stderr, _) = stderr_task
                .await
                .ok()
                .and_then(Result::ok)
                .ok_or_else(|| remote_failure("AWS CLI stderr could not be read"))?;
            Ok(CliOutput {
                success: status.success(),
                stdout,
                stdout_truncated,
                stderr,
            })
        })
    }
}

async fn read_bounded(
    mut reader: impl tokio::io::AsyncRead + Unpin,
    maximum: usize,
) -> std::io::Result<(Vec<u8>, bool)> {
    use tokio::io::AsyncReadExt as _;
    let mut bytes = Vec::new();
    let mut truncated = false;
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        let remaining = maximum.saturating_sub(bytes.len());
        let retained = remaining.min(read);
        bytes.extend_from_slice(&buffer[..retained]);
        truncated |= retained < read;
    }
    Ok((bytes, truncated))
}

pub(crate) fn map_spawn_failure(error: &std::io::Error) -> ConnectorFailure {
    if error.kind() == ErrorKind::NotFound {
        ConnectorFailure::not_found(safe_message("the aws CLI was not found on PATH"))
    } else {
        remote_failure("the aws CLI could not be spawned")
    }
}

fn descriptor() -> ConnectorDescriptor {
    ConnectorDescriptor::new("aws", "cli-1").expect("static AWS connector descriptor is valid")
}

fn capability() -> CapabilityName {
    CapabilityName::parse("commands.inspect").expect("static AWS capability is valid")
}

fn summary(value: String) -> Result<BoundedSummary, ConnectorFailure> {
    BoundedSummary::new(value)
        .map_err(|_| remote_failure("AWS connector summary exceeded SDK bounds"))
}

/// Builds a sanitized failure message carrying a control-free, bounded excerpt
/// of the CLI's stderr; credential material never reaches stderr for the
/// allowlisted read-only commands.
fn cli_failure_message(prefix: &str, stderr: &[u8]) -> FailureMessage {
    let mut excerpt = String::from_utf8_lossy(stderr)
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    let mut length = excerpt.len().min(MAX_STDERR_EXCERPT_BYTES);
    while !excerpt.is_char_boundary(length) {
        length -= 1;
    }
    excerpt.truncate(length);
    let excerpt = excerpt.trim();
    let message = if excerpt.is_empty() {
        prefix.to_owned()
    } else {
        format!("{prefix}: {excerpt}")
    };
    FailureMessage::new(message).unwrap_or_else(|_| safe_message(prefix))
}

fn invalid_request(message: &str) -> ConnectorFailure {
    ConnectorFailure::invalid_request(safe_message(message))
}

fn remote_failure(message: &str) -> ConnectorFailure {
    ConnectorFailure::remote(safe_message(message), false)
}

fn safe_message(value: &str) -> FailureMessage {
    FailureMessage::new(value).expect("static AWS failure message is valid")
}
