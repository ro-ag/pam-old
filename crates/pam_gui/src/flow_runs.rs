//! Running a global flow definition against one project, and observing the
//! run afterwards.
//!
//! The flow *library* is daemon-global; a run is not. Every run binds to one
//! project at invocation time, exactly as `pam flow run` binds the outer
//! project root while the catalog stays global. Observation reuses the generic
//! request capabilities the CLI already uses — `request.replay` with an
//! `after` cursor for progress, `request.result.read` for the terminal result,
//! and `request.cancel` for cancellation — so nothing flow-specific is added
//! to the protocol.

use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use pam_client::request_exchange;
use pam_core::{CallerCredential, CallerId, IdempotencyKey, ProjectId, RequestId};
use pam_flow::RunId;
use pam_platform::LocalEndpoint;
use pam_protocol::{
    ExpectedTargetKind, FlowProjectRoot, ProtocolContractError, RequestEnvelope, ResultBody,
    ResultPayload,
};
use pam_store::{FlowRunSummary, MAX_FLOW_RUN_HISTORY, Store};
use tokio::sync::Mutex;
use uuid::Uuid;

use serde::{Deserialize, Serialize};

use crate::{
    control_center::exchange_failure_context,
    current::{
        OutcomeView, TimelineFact, outcome_view, timeline_from_events, unique_idempotency,
        unique_request_id,
    },
    desktop::{CommandFence, OutcomeDto, TimelineFactDto},
};

/// The accepted run: its durable identity and the CLI line that resumes it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FlowRunDto {
    pub fence: CommandFence,
    pub data: FlowRunDataDto,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FlowRunDataDto {
    pub run_id: String,
    pub definition_id: String,
    /// The project this run is bound to. A run genuinely has a project; the
    /// flow library it came from does not.
    pub project_label: String,
    pub retry_command: String,
}

/// One bounded progress window over a run.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FlowRunProgressDto {
    pub fence: CommandFence,
    pub data: FlowRunProgressDataDto,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FlowRunProgressDataDto {
    pub run_id: String,
    /// The cursor to send as `after` on the next poll.
    pub cursor: u64,
    pub facts: Vec<TimelineFactDto>,
    pub truncated: bool,
    pub terminal: bool,
    pub outcome: Option<OutcomeDto>,
    pub detail_error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FlowRunCancelDto {
    pub fence: CommandFence,
    pub data: FlowRunCancelDataDto,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FlowRunCancelDataDto {
    pub run_id: String,
    pub disposition: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FlowRunHistoryDto {
    pub fence: CommandFence,
    pub data: FlowRunHistoryDataDto,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FlowRunHistoryDataDto {
    pub runs: Vec<FlowRunHistoryEntryDto>,
    /// The durable history is longer than the bound this read returns.
    pub truncated: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FlowRunHistoryEntryDto {
    pub run_id: String,
    pub definition_id: Option<String>,
    pub project_label: String,
    pub state: String,
    pub outcome: Option<String>,
    pub started_at_ms: u64,
    pub completed_at_ms: Option<u64>,
}

/// Bound on the transitions one progress read renders. The daemon's replay is
/// bounded on its own side; this is the GUI's own ceiling on a single window.
pub(crate) const MAX_RUN_FACTS: usize = 200;

/// Bound on the history list the GUI reads and renders.
pub(crate) const MAX_RUN_HISTORY: u32 = MAX_FLOW_RUN_HISTORY;

/// Read-side exchanges are interactive: a poll that cannot answer quickly is
/// reported and retried by the next poll.
const OBSERVE_TIMEOUT: Duration = Duration::from_secs(2);

/// Ceiling on how long a detached submission waits for its own terminal
/// result. Progress, outcome, and history all come from durable state, so this
/// only bounds how long the submitting task lingers.
const SUBMIT_TIMEOUT: Duration = Duration::from_hours(1);

/// One accepted run's identity, as the GUI reports it back to the view.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StartedRun {
    pub(crate) run_id: RequestId,
    pub(crate) idempotency_key: IdempotencyKey,
    pub(crate) project_label: String,
}

/// One bounded progress window over a run in flight.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct RunProgress {
    /// The replay cursor to send as `after` on the next poll.
    pub(crate) cursor: u64,
    pub(crate) facts: Vec<TimelineFact>,
    /// The daemon's own replay window overflowed this read's bound.
    pub(crate) truncated: bool,
    /// The run has reached a terminal result.
    pub(crate) terminal: bool,
    pub(crate) outcome: Option<OutcomeView>,
    pub(crate) detail_error: Option<String>,
}

/// One durable past run, named by the catalog entry it still matches.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RunHistoryEntry {
    pub(crate) run_id: String,
    /// The catalog definition whose normalized source still digests to the one
    /// this run executed. Absent once the definition has been edited away.
    pub(crate) definition_id: Option<String>,
    /// The project this run was bound to: its remembered canonical root when
    /// the daemon has learned one, otherwise its opaque ID.
    pub(crate) project_label: String,
    pub(crate) state: &'static str,
    pub(crate) outcome: Option<&'static str>,
    pub(crate) started_at_ms: u64,
    pub(crate) completed_at_ms: Option<u64>,
}

/// Builds the `flow.run` envelope for one global definition against one
/// project.
///
/// The run identity is minted exactly as `pam flow run` mints it: one fresh
/// UUID becomes the run ID, and the idempotency key is *derived from that run
/// ID* rather than drawn independently. A retry that reuses the pair is the
/// same operation to the daemon; two runs can never collide on a key while
/// carrying different run IDs.
///
/// # Errors
///
/// Returns a contract error when the definition or the project root is
/// malformed or over budget.
pub(crate) fn run_request(
    caller_id: CallerId,
    credential: CallerCredential,
    project_id: ProjectId,
    project_root: &str,
    definition: String,
) -> Result<(RequestEnvelope, StartedRun), ProtocolContractError> {
    let run_id = RequestId::new(format!("flow-run-{}", Uuid::new_v4()));
    let idempotency_key = IdempotencyKey::new(format!("flow-run:{run_id}"));
    let request = RequestEnvelope::flow_run(
        run_id.clone(),
        caller_id,
        project_id,
        idempotency_key.clone(),
        definition,
        project_root,
    )?
    .authenticated(credential)
    .with_project_root(FlowProjectRoot::new(project_root)?);
    Ok((
        request,
        StartedRun {
            run_id,
            idempotency_key,
            project_label: project_root.to_owned(),
        },
    ))
}

/// The retry line for one run, identical in form to the CLI's own.
pub(crate) fn retry_command(definition_id: &str, started: &StartedRun) -> String {
    format!(
        "pam flow run {definition_id} --run-id {} --idempotency-key {}",
        started.run_id, started.idempotency_key
    )
}

/// Parses a run identifier handed back by the view.
pub(crate) fn parse_run_id(value: &str) -> Option<RequestId> {
    RunId::parse(value).ok().map(|_| RequestId::new(value))
}

/// Outcomes of detached submissions this session started.
///
/// A run the daemon refuses outright — a policy deny, a required approval, or
/// a transport failure — never reaches durable state, so replay alone would
/// leave the view polling an ID that will never exist. The refusal is kept
/// here, bounded, until the view reads it.
#[derive(Clone, Debug, Default)]
pub(crate) struct FlowRunSubmissions {
    inner: Arc<Mutex<VecDeque<(RequestId, String)>>>,
}

impl FlowRunSubmissions {
    const CAPACITY: usize = 8;

    /// Sends one accepted flow run without waiting for it.
    ///
    /// The run's events, outcome, and evidence are all durable, so the caller
    /// observes the run through replay rather than through this exchange; only
    /// a refusal that leaves nothing durable is retained.
    pub(crate) fn submit(&self, request: RequestEnvelope) {
        self.submit_to(LocalEndpoint::default_for_user(), request);
    }

    pub(crate) fn submit_to(&self, endpoint: LocalEndpoint, request: RequestEnvelope) {
        let submissions = self.clone();
        let run_id = request.request_id.clone();
        tokio::spawn(async move {
            let detail = match request_exchange(&endpoint, &request, SUBMIT_TIMEOUT).await {
                Ok(exchange) => match exchange.result.body {
                    ResultBody::Failure(failure) => Some(failure.message),
                    ResultBody::Success { .. } => None,
                },
                Err(error) => Some(exchange_failure_context(&error).0),
            };
            if let Some(detail) = detail {
                submissions.record(run_id, detail).await;
            }
        });
    }

    async fn record(&self, run_id: RequestId, detail: String) {
        let mut inner = self.inner.lock().await;
        inner.push_back((run_id, detail));
        while inner.len() > Self::CAPACITY {
            inner.pop_front();
        }
    }

    pub(crate) async fn failure(&self, run_id: &RequestId) -> Option<String> {
        let inner = self.inner.lock().await;
        inner
            .iter()
            .find(|(candidate, _)| candidate == run_id)
            .map(|(_, detail)| detail.clone())
    }
}

/// Reads one bounded progress window for a run, and its terminal result once
/// the run has one.
///
/// `after` is the replay cursor from the previous window; zero replays the run
/// from its first event.
pub(crate) async fn observe(
    caller_id: CallerId,
    credential: CallerCredential,
    project_id: ProjectId,
    run_id: RequestId,
    after: u64,
) -> RunProgress {
    observe_at(
        &LocalEndpoint::default_for_user(),
        caller_id,
        credential,
        project_id,
        run_id,
        after,
    )
    .await
}

pub(crate) async fn observe_at(
    endpoint: &LocalEndpoint,
    caller_id: CallerId,
    credential: CallerCredential,
    project_id: ProjectId,
    run_id: RequestId,
    after: u64,
) -> RunProgress {
    let replay = RequestEnvelope::replay_with_expected_target(
        unique_request_id("gui-flow-replay"),
        caller_id.clone(),
        project_id.clone(),
        unique_idempotency("gui-flow-replay"),
        run_id.clone(),
        after,
        ExpectedTargetKind::FlowRun,
    )
    .authenticated(credential.clone());
    let exchange = match request_exchange(endpoint, &replay, OBSERVE_TIMEOUT).await {
        Ok(exchange) => exchange,
        Err(error) => {
            return RunProgress {
                cursor: after,
                detail_error: Some(exchange_failure_context(&error).0),
                ..RunProgress::default()
            };
        }
    };
    let replayed = match exchange.result.body {
        ResultBody::Success {
            payload: ResultPayload::Replay(replayed),
            ..
        } if replayed.target_request_id == run_id => replayed,
        ResultBody::Failure(failure) => {
            return RunProgress {
                cursor: after,
                detail_error: Some(failure.message),
                ..RunProgress::default()
            };
        }
        ResultBody::Success { .. } => {
            return RunProgress {
                cursor: after,
                detail_error: Some("PAM returned an unexpected flow replay response.".to_owned()),
                ..RunProgress::default()
            };
        }
    };
    let mut facts = timeline_from_events(&exchange.events);
    let truncated = facts.len() > MAX_RUN_FACTS;
    facts.truncate(MAX_RUN_FACTS);
    let mut progress = RunProgress {
        cursor: replayed.through_sequence.max(after),
        facts,
        truncated,
        terminal: !replayed.pending,
        outcome: None,
        detail_error: None,
    };
    if progress.terminal {
        match terminal_result(endpoint, caller_id, credential, project_id, run_id).await {
            Ok(outcome) => progress.outcome = Some(outcome),
            Err(detail) => progress.detail_error = Some(detail),
        }
    }
    progress
}

async fn terminal_result(
    endpoint: &LocalEndpoint,
    caller_id: CallerId,
    credential: CallerCredential,
    project_id: ProjectId,
    run_id: RequestId,
) -> Result<OutcomeView, String> {
    let request = RequestEnvelope::get_result_with_expected_target(
        unique_request_id("gui-flow-result"),
        caller_id,
        project_id,
        unique_idempotency("gui-flow-result"),
        run_id,
        ExpectedTargetKind::FlowRun,
    )
    .authenticated(credential);
    let exchange = request_exchange(endpoint, &request, OBSERVE_TIMEOUT)
        .await
        .map_err(|error| exchange_failure_context(&error).0)?;
    match exchange.result.body {
        ResultBody::Success {
            payload: ResultPayload::FlowRun(result),
            ..
        } if exchange.events.is_empty() => Ok(outcome_view(&result)),
        ResultBody::Failure(failure) => Err(failure.message),
        ResultBody::Success { .. } => Err("PAM returned an unexpected flow result.".to_owned()),
    }
}

/// Requests cancellation of one run in flight, at the next safe boundary.
pub(crate) async fn cancel(
    caller_id: CallerId,
    credential: CallerCredential,
    project_id: ProjectId,
    run_id: RequestId,
) -> Result<&'static str, String> {
    cancel_at(
        &LocalEndpoint::default_for_user(),
        caller_id,
        credential,
        project_id,
        run_id,
    )
    .await
}

pub(crate) async fn cancel_at(
    endpoint: &LocalEndpoint,
    caller_id: CallerId,
    credential: CallerCredential,
    project_id: ProjectId,
    run_id: RequestId,
) -> Result<&'static str, String> {
    let request = RequestEnvelope::cancel_with_expected_target(
        unique_request_id("gui-flow-cancel"),
        caller_id,
        project_id,
        unique_idempotency("gui-flow-cancel"),
        run_id.clone(),
        ExpectedTargetKind::FlowRun,
    )
    .authenticated(credential);
    let exchange = request_exchange(endpoint, &request, OBSERVE_TIMEOUT)
        .await
        .map_err(|error| exchange_failure_context(&error).0)?;
    match exchange.result.body {
        ResultBody::Success {
            payload: ResultPayload::Cancellation(result),
            ..
        } if result.target_request_id == run_id => Ok(cancellation_label(&result.disposition)),
        ResultBody::Failure(failure) => Err(failure.message),
        ResultBody::Success { .. } => {
            Err("PAM returned an unexpected cancellation response.".to_owned())
        }
    }
}

const fn cancellation_label(disposition: &pam_protocol::CancellationDisposition) -> &'static str {
    match disposition {
        pam_protocol::CancellationDisposition::Requested => "requested",
        pam_protocol::CancellationDisposition::AlreadyRequested => "already_requested",
        pam_protocol::CancellationDisposition::AlreadyCancelled => "already_cancelled",
        pam_protocol::CancellationDisposition::AlreadyTerminal => "already_terminal",
    }
}

/// Reads the bounded newest-first run history straight from durable state.
///
/// `catalog` maps a definition digest to the catalog ID that still carries it,
/// so a run can be named by the flow it executed without re-reading the
/// definition it ran.
pub(crate) async fn history(
    state_path: PathBuf,
    catalog: &HashMap<[u8; 32], String>,
) -> Result<Vec<RunHistoryEntry>, String> {
    let store = Store::open(state_path).map_err(|error| error.to_string())?;
    let runs = store.recent_flow_runs(MAX_RUN_HISTORY).await;
    let shutdown = store.shutdown().await;
    let runs = runs.map_err(|error| error.to_string())?;
    shutdown.map_err(|error| error.to_string())?;
    Ok(runs
        .into_iter()
        .map(|run| history_entry(run, catalog))
        .collect())
}

fn history_entry(run: FlowRunSummary, catalog: &HashMap<[u8; 32], String>) -> RunHistoryEntry {
    RunHistoryEntry {
        run_id: run.request_id.as_str().to_owned(),
        definition_id: catalog.get(&run.definition_digest).cloned(),
        project_label: run
            .project_root
            .unwrap_or_else(|| run.project_id.as_str().to_owned()),
        state: state_label(run.state),
        outcome: run.outcome.map(outcome_label),
        started_at_ms: run.accepted_at_ms,
        completed_at_ms: run.completed_at_ms,
    }
}

const fn state_label(state: pam_store::RequestState) -> &'static str {
    match state {
        pam_store::RequestState::Queued => "queued",
        pam_store::RequestState::Leased => "leased",
        pam_store::RequestState::CancellationRequested => "cancellation_requested",
        pam_store::RequestState::Succeeded => "succeeded",
        pam_store::RequestState::Failed => "failed",
        pam_store::RequestState::Cancelled => "cancelled",
    }
}

const fn outcome_label(outcome: pam_flow::RunOutcome) -> &'static str {
    match outcome {
        pam_flow::RunOutcome::Solved => "solved",
        pam_flow::RunOutcome::Unresolved => "unresolved",
        pam_flow::RunOutcome::Blocked => "blocked",
        pam_flow::RunOutcome::Cancelled => "cancelled",
    }
}

#[cfg(test)]
pub(crate) fn history_entry_for_test(
    run: FlowRunSummary,
    catalog: &HashMap<[u8; 32], String>,
) -> RunHistoryEntry {
    history_entry(run, catalog)
}
