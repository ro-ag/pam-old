use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use pam_client::request_exchange;
use pam_core::{CallerCredential, CallerId, GrantId, ProjectId, RequestId};
use pam_daemon::{DaemonConfig, serve_until};
use pam_flow::RunOutcome;
use pam_platform::LocalEndpoint;
use pam_policy::{ApprovalRequirement, CapabilityName, Effect, Grant, ResourceScope};
use pam_protocol::{Capability, FailureCode, RequestEnvelope, RequestPayload, ResultBody};
use pam_store::{FlowRunSummary, PutGrant, RequestState, Store};
use tokio::sync::oneshot;

use super::*;

static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

const TEST_CALLER: &str = "gui-flow-run-caller";
const TEST_CREDENTIAL: &str = "gui-flow-run-credential";
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(10);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(label: &str) -> Self {
        let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        // Unix domain sockets cap the whole path at ~104 bytes, and macOS
        // temp directories are far too deep for that.
        let base = if cfg!(unix) {
            PathBuf::from("/tmp")
        } else {
            std::env::temp_dir()
        };
        let path = base.join(format!(
            "pam-gui-fr-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(fs::canonicalize(&path).unwrap())
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Writes the explicit project marker so `discover_project` resolves this
/// directory to a stable ID — the daemon re-derives it from the run's project
/// root before it admits the run — and initializes the git workspace the
/// daemon fingerprints before it accepts one.
fn mark_project(root: &Path) -> ProjectId {
    assert!(
        std::process::Command::new("git")
            .args(["-c", "init.defaultBranch=main", "init", "--quiet"])
            .current_dir(root)
            .status()
            .unwrap()
            .success()
    );
    let project_id = uuid::Uuid::new_v4().to_string();
    let marker = root.join(".pam");
    fs::create_dir_all(&marker).unwrap();
    fs::write(
        marker.join("project.toml"),
        format!("version = 1\nproject_id = \"{project_id}\"\n"),
    )
    .unwrap();
    ProjectId::new(project_id)
}

fn flow_source(id: &str) -> String {
    format!(
        r#"schema_version = 2
id = "{id}"
name = "Bounded run"
description = "A bounded run for the GUI run surface."
revision = 1

[outcome]
solved = "Solved."
changed = "Changed."
verified = "Verified."
unresolved = "Unresolved."
blocked = "Blocked."

[[steps]]
id = "inspect"
description = "Inspect the worktree."
timeout_seconds = 10
effect = "read_only"
semantic = "observe"
action = {{ type = "command", program = "git", args = ["status", "--short"], working_directory = "." }}
"#
    )
}

fn start_daemon(
    endpoint: LocalEndpoint,
) -> (
    oneshot::Sender<()>,
    tokio::task::JoinHandle<Result<(), pam_daemon::DaemonError>>,
) {
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let state_path = endpoint.runtime_dir().join("state.sqlite3");
    let daemon = tokio::spawn(serve_until(
        DaemonConfig {
            endpoint,
            recover: false,
            model: None,
            state_path: Some(state_path),
            brief_provider: None,
            connector_secret_backend: None,
        },
        async {
            let _ = shutdown_rx.await;
        },
    ));
    (shutdown_tx, daemon)
}

async fn wait_until_ready(endpoint: &LocalEndpoint) {
    for _ in 0..200 {
        if endpoint.socket_path().is_some_and(Path::exists) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Registers the caller, and — when the run is meant to be admitted — grants
/// the one run capability plus the generic observation capabilities the CLI
/// already uses for the same job.
async fn seed(state_path: &Path, project_id: &ProjectId, grant_flow_run: bool) {
    let store = Store::open(state_path).unwrap();
    store
        .register_caller(
            CallerId::from(TEST_CALLER),
            CallerCredential::new(TEST_CREDENTIAL),
            1,
        )
        .await
        .unwrap();
    if grant_flow_run {
        for (index, capability) in [
            "flow.run",
            "request.replay",
            "request.result.read",
            "request.cancel",
        ]
        .into_iter()
        .enumerate()
        {
            store
                .put_grant(PutGrant {
                    grant: Grant {
                        id: GrantId::from(format!("gui-flow-run-grant-{index}")),
                        caller: CallerId::from(TEST_CALLER),
                        project: project_id.clone(),
                        capability: CapabilityName::parse(capability).unwrap(),
                        resource: ResourceScope::Any,
                        effect: Effect::Allow,
                        approval: ApprovalRequirement::None,
                        expires_at_ms: None,
                        revoked_at_ms: None,
                    },
                    created_at_ms: 2,
                })
                .await
                .unwrap();
        }
    }
    store.shutdown().await.unwrap();
}

fn started(project_root: &str, project_id: ProjectId) -> (RequestEnvelope, flow_runs::StartedRun) {
    flow_runs::run_request(
        CallerId::from(TEST_CALLER),
        CallerCredential::new(TEST_CREDENTIAL),
        project_id,
        project_root,
        flow_source("gui-run"),
    )
    .unwrap()
}

#[test]
fn the_idempotency_key_is_derived_from_the_run_id_it_belongs_to() {
    let project = TestDirectory::new("identity");
    let project_id = mark_project(project.path());
    let root = project.path().to_str().unwrap();

    let (first, first_started) = started(root, project_id.clone());
    let (second, second_started) = started(root, project_id);

    // Two runs never share a run ID, and each key is bound to its own run: a
    // retry that reuses the pair is the same operation, and two runs can never
    // collide on one key while carrying different run IDs.
    assert_ne!(first.request_id, second.request_id);
    assert_ne!(first.idempotency_key, second.idempotency_key);
    assert_eq!(
        first.idempotency_key.as_str(),
        format!("flow-run:{}", first.request_id)
    );
    assert!(first.request_id.as_str().starts_with("flow-run-"));
    assert_eq!(first.capability, Capability::FlowRun);
    assert!(matches!(first.payload, RequestPayload::FlowRun { .. }));
    assert!(first.authentication.is_some());
    // The definition validates as a run identity, which the daemon re-checks.
    first.validate_flow_request().unwrap();

    assert_eq!(
        flow_runs::retry_command("gui-run", &first_started),
        format!(
            "pam flow run gui-run --run-id {} --idempotency-key {}",
            first_started.run_id, first_started.idempotency_key
        )
    );
    assert_ne!(first_started.run_id, second_started.run_id);
}

#[test]
fn only_canonical_run_identifiers_are_accepted_back_from_the_view() {
    let project = TestDirectory::new("run-id");
    let project_id = mark_project(project.path());
    let (request, _) = started(project.path().to_str().unwrap(), project_id);

    assert_eq!(
        flow_runs::parse_run_id(request.request_id.as_str()),
        Some(request.request_id.clone())
    );
    for rejected in ["", " ", "flow run", "flow\nrun", &"a".repeat(4096)] {
        assert_eq!(flow_runs::parse_run_id(rejected), None, "{rejected:?}");
    }
}

#[test]
fn history_names_a_run_by_the_catalog_entry_it_still_matches() {
    let mut catalog = HashMap::new();
    catalog.insert([7_u8; 32], "after-merge-checks".to_owned());

    let known = flow_runs::history_entry_for_test(
        FlowRunSummary {
            request_id: RequestId::from("flow-run-1"),
            project_id: ProjectId::from("project-1"),
            project_root: Some("/work/repo".to_owned()),
            state: RequestState::Succeeded,
            definition_digest: [7_u8; 32],
            outcome: Some(RunOutcome::Solved),
            accepted_at_ms: 10,
            updated_at_ms: 20,
            completed_at_ms: Some(20),
        },
        &catalog,
    );
    assert_eq!(known.definition_id.as_deref(), Some("after-merge-checks"));
    // A run genuinely has a project; the label is the run's, never the view's.
    assert_eq!(known.project_label, "/work/repo");
    assert_eq!(known.state, "succeeded");
    assert_eq!(known.outcome, Some("solved"));

    // An edited-away definition still lists its run, unnamed, and a project the
    // daemon only knows by ID falls back to that ID.
    let unknown = flow_runs::history_entry_for_test(
        FlowRunSummary {
            request_id: RequestId::from("flow-run-2"),
            project_id: ProjectId::from("project-2"),
            project_root: None,
            state: RequestState::Leased,
            definition_digest: [9_u8; 32],
            outcome: None,
            accepted_at_ms: 30,
            updated_at_ms: 31,
            completed_at_ms: None,
        },
        &catalog,
    );
    assert_eq!(unknown.definition_id, None);
    assert_eq!(unknown.project_label, "project-2");
    assert_eq!(unknown.state, "leased");
    assert_eq!(unknown.outcome, None);
}

/// The authorization boundary, proven with the exact envelope the GUI sends,
/// through a real daemon rather than against a DTO.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_run_envelope_is_refused_without_a_flow_run_grant() {
    let runtime = TestDirectory::new("denied-runtime");
    let project = TestDirectory::new("denied-project");
    let project_id = mark_project(project.path());
    let endpoint = LocalEndpoint::ipc(runtime.path().to_path_buf());
    seed(
        &endpoint.runtime_dir().join("state.sqlite3"),
        &project_id,
        false,
    )
    .await;

    let (shutdown, daemon) = start_daemon(endpoint.clone());
    wait_until_ready(&endpoint).await;

    let (request, _) = started(project.path().to_str().unwrap(), project_id);
    let exchange = request_exchange(&endpoint, &request, EXCHANGE_TIMEOUT)
        .await
        .unwrap();
    match exchange.result.body {
        ResultBody::Failure(failure) => {
            assert_eq!(failure.code, FailureCode::Forbidden, "{failure:?}");
        }
        other @ ResultBody::Success { .. } => {
            panic!("an ungranted flow run must be refused: {other:?}")
        }
    }

    let _ = shutdown.send(());
    let _ = daemon.await;
}

/// The same envelope, granted: the daemon admits it, the run reaches a
/// terminal result, and the generic observation path reports it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_granted_run_is_admitted_and_observed_through_replay_and_result() {
    let runtime = TestDirectory::new("granted-runtime");
    let project = TestDirectory::new("granted-project");
    let project_id = mark_project(project.path());
    let endpoint = LocalEndpoint::ipc(runtime.path().to_path_buf());
    seed(
        &endpoint.runtime_dir().join("state.sqlite3"),
        &project_id,
        true,
    )
    .await;

    let (shutdown, daemon) = start_daemon(endpoint.clone());
    wait_until_ready(&endpoint).await;

    let (request, run) = started(project.path().to_str().unwrap(), project_id.clone());
    let run_id = run.run_id.clone();
    let exchange = request_exchange(&endpoint, &request, EXCHANGE_TIMEOUT)
        .await
        .unwrap();
    if let ResultBody::Failure(failure) = &exchange.result.body {
        assert_ne!(
            failure.code,
            FailureCode::Forbidden,
            "a granted flow run must not be refused: {failure:?}"
        );
    }

    // The run is durable now, so the generic replay/result path answers for it
    // exactly as it answers for the CLI.
    let progress = flow_runs::observe_at(
        &endpoint,
        CallerId::from(TEST_CALLER),
        CallerCredential::new(TEST_CREDENTIAL),
        project_id.clone(),
        run_id.clone(),
        0,
    )
    .await;
    assert_eq!(progress.detail_error, None);
    assert!(progress.terminal, "the submitted run reached a result");
    assert!(
        progress.outcome.is_some(),
        "the terminal result is readable"
    );
    assert!(
        !progress.facts.is_empty(),
        "the run replayed its transitions"
    );
    assert!(progress.facts.len() <= flow_runs::MAX_RUN_FACTS);

    // Cancelling a run that is already terminal is answered, not an error.
    let disposition = flow_runs::cancel_at(
        &endpoint,
        CallerId::from(TEST_CALLER),
        CallerCredential::new(TEST_CREDENTIAL),
        project_id,
        run_id,
    )
    .await
    .unwrap();
    assert_eq!(disposition, "already_terminal");

    // The same run is in the durable history, labelled with its project.
    let history = flow_runs::history(
        endpoint.runtime_dir().join("state.sqlite3"),
        &HashMap::new(),
    )
    .await
    .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].run_id, run.run_id.as_str());
    assert_eq!(history[0].project_label, project.path().to_str().unwrap());

    let _ = shutdown.send(());
    let _ = daemon.await;
}
