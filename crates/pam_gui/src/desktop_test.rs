use pam_core::{
    ApprovalId, CallerCredential, CallerId, ContentDigest, EvidenceHandle, IdempotencyKey,
    ProjectId, RequestId,
};
use pam_protocol::{
    ActivityEventSummary, ActivityResult, ApprovalChallenge, CallerListResult, CallerSummary,
    ConfigurationPresence, ConnectorConfigureResult, ConnectorListResult, ConnectorSummary,
    ConnectorTestDisposition, ConnectorTestResult, FailureCode, MAX_MODEL_OUTPUT_TOKENS,
    ModelFinishReason, ModelGenerationResult, ModelStatusResult, ModelSummary, ModelUsage,
    NetworkDiagnosticsResult, OperationTruth, PacState, RequestEnvelope,
};
use pam_skills::CanonicalEntryId;
use std::sync::atomic::AtomicUsize;

use super::{
    access_config::{AccessConfigState, map_diagnostics_for_test},
    control_center::HealthState,
    current::{CurrentState, EvidencePreview, pending_approval_for_test},
    desktop::{
        AccessConfigDto, AppSettingsDto, ApprovalDecisionDispositionDto, BootstrapDto, CatalogDto,
        CommandFence, ConnectorConfigureParams, CurrentDto, DaemonStartup, DesktopCore,
        DesktopErrorKind, DesktopResult, EvidenceHandleDto, FailureDto, FailureKindDto,
        FlowComposeDto, FlowDefinitionHandle, FlowDocumentHandle, FlowGraphDto, GenerationId,
        HealthDto, ModelDownloadDto, ModelInspectDto, ModelStatusDto, ModelSummaryDto, OperationId,
        ProjectHandle, TimelineKindDto, access_dto_for_test, active_core_at_for_test,
        active_core_for_test, activity_dto_for_test, approval_current_for_test,
        approval_failure_retains_handle_for_test, bootstrap_with_catalog_for_test,
        bounded_detail_for_test, callers_dto_for_test, clamp_model_output_tokens_for_test,
        command_gate_for_test, connector_configure_dto_for_test, connector_test_dto_for_test,
        connectors_dto_for_test, current_dto_for_test, daemon_start_cwd_for_test,
        evidence_dto_for_test, failure_kind_for_test, flow_compose_data_for_test,
        flow_graph_data_for_test, flow_workspace_at_for_test, gui_registration_current_for_test,
        manage_skill_library_without_io_for_test, mark_model_loading_for_test,
        model_infer_dto_for_test, model_status_dto_for_test, post_save_reload_error_for_test,
        reap_daemon_child_for_test, registered_model_catalog_in_for_test,
        registration_contract_for_test, registration_failure_detail, replace_daemon_child_for_test,
        reserve_daemon_for_test, reserve_for_test, startup_budget_for_bytes_for_test,
        switch_authority_for_test, wait_for_daemon_serving_for_test,
    },
    flow_editor::FlowEditorError,
    observatory::ObservatoryState,
    skill_library::{SkillLibraryAgentDto, SkillLibraryRequest},
};

#[test]
fn timeline_kinds_have_stable_exhaustive_wire_names() {
    let kinds = [
        TimelineKindDto::Request,
        TimelineKindDto::Evidence,
        TimelineKindDto::Change,
        TimelineKindDto::Verification,
        TimelineKindDto::Failure,
    ];

    assert_eq!(
        serde_json::to_value(kinds).unwrap(),
        serde_json::json!(["request", "evidence", "change", "verification", "failure"])
    );
}

#[test]
fn approval_dispositions_have_stable_truthful_wire_names() {
    assert_eq!(
        serde_json::to_value([
            ApprovalDecisionDispositionDto::Approved,
            ApprovalDecisionDispositionDto::Denied,
            ApprovalDecisionDispositionDto::Expired,
        ])
        .unwrap(),
        serde_json::json!(["approved", "denied", "expired"])
    );
}

#[test]
fn gui_registration_recovery_uses_a_stable_typed_current_code() {
    let current = gui_registration_current_for_test("Native credential is unavailable.".to_owned());

    assert_eq!(
        serde_json::to_value(current).unwrap(),
        serde_json::json!({
            "status": "unavailable",
            "failure": {
                "kind": "unavailable",
                "code": "gui_registration_required",
                "detail": "Native credential is unavailable.",
                "recovery": "Use Register GUI caller in PAM."
            }
        })
    );
}

#[test]
fn tagged_desktop_dtos_serialize_variant_fields_in_the_frontend_contract() {
    assert_eq!(
        serde_json::to_value(HealthDto::Healthy {
            daemon_version: "0.1.0".to_owned(),
            queue_depth: 3,
        })
        .unwrap(),
        serde_json::json!({
            "status": "healthy",
            "daemonVersion": "0.1.0",
            "queueDepth": 3
        })
    );

    let available = access_dto_for_test(AccessConfigState::Available(map_diagnostics_for_test(
        OperationTruth::Observed,
        &NetworkDiagnosticsResult {
            platform_roots_enabled: true,
            system_proxy_discovery_enabled: false,
            proxy_environment_presence: ConfigurationPresence::Configured,
            no_proxy_presence: ConfigurationPresence::NotConfigured,
            pac_state: PacState::DetectedUnsupported,
        },
    )));
    assert_eq!(
        serde_json::to_value(available).unwrap(),
        serde_json::json!({
            "status": "available",
            "truth": "observed",
            "platformRootsEnabled": true,
            "systemProxyDiscoveryEnabled": false,
            "proxyEnvironment": "configured",
            "noProxy": "not configured",
            "pac": "detected but unsupported"
        })
    );

    assert_eq!(
        serde_json::to_value(AccessConfigDto::Blocked {
            failure: FailureDto {
                kind: FailureKindDto::Blocked,
                code: Some("forbidden".to_owned()),
                detail: "Blocked by policy.".to_owned(),
                recovery: None,
            },
            approval_id: Some("approval-1".to_owned()),
            expires_at_ms: Some(42),
        })
        .unwrap(),
        serde_json::json!({
            "status": "blocked",
            "failure": {
                "kind": "blocked",
                "code": "forbidden",
                "detail": "Blocked by policy.",
                "recovery": null
            },
            "approvalId": "approval-1",
            "expiresAtMs": 42
        })
    );

    assert_eq!(
        serde_json::to_value(ModelDownloadDto::Ok).unwrap(),
        serde_json::json!({ "status": "ok" })
    );
    assert_eq!(
        serde_json::to_value(ModelDownloadDto::Unavailable {
            failure: FailureDto {
                kind: FailureKindDto::Unavailable,
                code: Some("unknown_preset".to_owned()),
                detail: "This preset is not offered by PAM.".to_owned(),
                recovery: None,
            },
        })
        .unwrap(),
        serde_json::json!({
            "status": "unavailable",
            "failure": {
                "kind": "unavailable",
                "code": "unknown_preset",
                "detail": "This preset is not offered by PAM.",
                "recovery": null
            }
        })
    );
}

#[test]
fn post_save_refresh_failure_truthfully_reports_that_publication_succeeded() {
    let error = post_save_reload_error_for_test(&FlowEditorError::TooManyRecoveryArtifacts);

    assert_eq!(error.kind, DesktopErrorKind::Unavailable);
    assert!(error.message.starts_with("The flow was saved, but "));
    assert_eq!(
        error.recovery.as_deref(),
        Some("Reload the flow workspace before opening or saving another definition.")
    );
}

#[test]
fn desktop_detail_is_bounded_on_a_utf8_boundary() {
    let bounded = bounded_detail_for_test("é".repeat(3_000));

    assert!(bounded.len() <= 4 * 1024);
    assert!(bounded.ends_with("..."));
}

#[test]
fn gui_registration_uses_only_the_bundled_helper_and_fixed_bounded_contract() {
    let executable = std::path::Path::new("/Applications/PAM.app/Contents/MacOS/pam");
    let root = std::path::Path::new("/projects/pam");

    let (program, args, current_dir, kill_on_drop, timeout) =
        registration_contract_for_test(executable, root);

    assert_eq!(program, executable);
    assert_eq!(
        args,
        ["caller", "register", "--kind", "gui"]
            .map(std::ffi::OsString::from)
            .to_vec()
    );
    assert_eq!(current_dir.as_deref(), Some(root));
    assert!(kill_on_drop);
    assert_eq!(timeout, std::time::Duration::from_secs(15));
}

#[cfg(unix)]
#[test]
fn registration_failure_detail_surfaces_the_helper_reason_or_exit_status() {
    use std::os::unix::process::ExitStatusExt;
    let failed = std::process::ExitStatus::from_raw(2 << 8);

    let with_reason = std::process::Output {
        status: failed,
        stdout: Vec::new(),
        stderr: b"PAM's native credential store is unavailable.\nRecovery: pam caller register\n"
            .to_vec(),
    };
    assert_eq!(
        registration_failure_detail(&with_reason),
        "PAM GUI caller registration failed: PAM's native credential store is unavailable."
    );

    let silent = std::process::Output {
        status: failed,
        stdout: Vec::new(),
        stderr: b"  \n".to_vec(),
    };
    assert_eq!(
        registration_failure_detail(&silent),
        format!("PAM GUI caller registration failed with {failed}.")
    );
}

#[tokio::test]
async fn approved_current_is_preserved_without_a_second_project_current_load() {
    let calls = AtomicUsize::new(0);
    let approved = CurrentState::Available(super::current::CurrentView {
        queued: Vec::new(),
        truncated: false,
        run: None,
    });

    let refreshed = approval_current_for_test(approved.clone(), &calls).await;

    assert_eq!(refreshed, approved);
    assert_eq!(calls.into_inner(), 1);
}

#[tokio::test]
async fn denied_current_is_preserved_without_a_second_project_current_load() {
    let calls = AtomicUsize::new(0);
    let denied = CurrentState::Degraded {
        code: None,
        detail: "This exact project-current request was denied.".to_owned(),
        recovery: None,
    };

    let refreshed = approval_current_for_test(denied.clone(), &calls).await;

    assert_eq!(refreshed, denied);
    assert_eq!(calls.into_inner(), 1);
}

#[tokio::test]
async fn indeterminate_approval_failure_retains_the_exact_retry_handle() {
    let project = ProjectHandle::new();
    let generation = GenerationId::new();
    let core = active_core_for_test(&project, generation.clone());
    let fence = CommandFence::new(project, generation, OperationId::new());
    let approval = super::desktop::ApprovalHandle::new();
    let pending = pending_approval_for_test(
        RequestEnvelope::project_current(
            RequestId::new("current-retry"),
            CallerId::new("gui-retry"),
            ProjectId::new("internal-project-authority"),
            IdempotencyKey::new("current-retry"),
        )
        .authenticated(CallerCredential::new("credential-secret")),
        ApprovalChallenge {
            approval_id: ApprovalId::new("approval-retry"),
            expires_at_unix_ms: 100,
        },
    );

    let (error, retained, retry_authorized) =
        approval_failure_retains_handle_for_test(&core, fence, approval, pending).await;

    assert!(retained);
    assert!(retry_authorized);
    assert_eq!(error.kind, DesktopErrorKind::Unavailable);
    assert_eq!(error.message, "The approval response was not observed.");
    assert!(!format!("{error:?}").contains("credential-secret"));
}

#[test]
fn desktop_failure_mapping_blocks_only_policy_failures() {
    for code in [FailureCode::Forbidden, FailureCode::ApprovalRequired] {
        assert_eq!(failure_kind_for_test(&code), FailureKindDto::Blocked);
    }
    for code in [
        FailureCode::Unauthenticated,
        FailureCode::ApprovalDenied,
        FailureCode::ApprovalExpired,
        FailureCode::UnsupportedProtocolVersion,
        FailureCode::InvalidRequest,
        FailureCode::FrameTooLarge,
        FailureCode::NotFound,
        FailureCode::Pending,
        FailureCode::IdempotencyConflict,
        FailureCode::Cancelled,
        FailureCode::LeaseConflict,
        FailureCode::Busy,
        FailureCode::Internal,
    ] {
        assert_eq!(failure_kind_for_test(&code), FailureKindDto::Unavailable);
    }
}

#[tokio::test]
async fn operation_generation_and_project_switches_are_fenced() {
    let project = ProjectHandle::new();
    let generation = GenerationId::new();
    let operation = OperationId::new();
    let core = active_core_for_test(&project, generation.clone());
    let fence = CommandFence::new(project.clone(), generation.clone(), operation.clone());

    reserve_for_test(&core, &fence).await.unwrap();
    assert!(reserve_for_test(&core, &fence).await.is_err());

    let new_generation = GenerationId::new();
    switch_authority_for_test(&core, project.clone(), new_generation.clone()).await;
    let stale_generation = CommandFence::new(project.clone(), generation, OperationId::new());
    assert!(reserve_for_test(&core, &stale_generation).await.is_err());

    let other_project = ProjectHandle::new();
    switch_authority_for_test(&core, other_project, new_generation.clone()).await;
    let stale_project = CommandFence::new(project, new_generation, OperationId::new());
    assert!(reserve_for_test(&core, &stale_project).await.is_err());
}

#[tokio::test]
async fn skill_inventory_rejects_a_stale_project_before_scanning() {
    let project = ProjectHandle::new();
    let generation = GenerationId::new();
    let core = active_core_for_test(&project, generation.clone());
    let stale = CommandFence::new(project.clone(), generation, OperationId::new());
    switch_authority_for_test(&core, ProjectHandle::new(), GenerationId::new()).await;

    let error = core.skill_inventory(stale).await.unwrap_err();

    assert_eq!(error.kind, DesktopErrorKind::Stale);
}

#[tokio::test]
async fn skill_audit_commands_reject_a_stale_project_before_storage_or_scan_work() {
    let project = ProjectHandle::new();
    let generation = GenerationId::new();
    let core = active_core_for_test(&project, generation.clone());
    switch_authority_for_test(&core, ProjectHandle::new(), GenerationId::new()).await;

    let load_error = core
        .load_skill_audit(CommandFence::new(
            project.clone(),
            generation.clone(),
            OperationId::new(),
        ))
        .await
        .unwrap_err();
    let run_error = core
        .run_skill_audit(CommandFence::new(project, generation, OperationId::new()))
        .await
        .unwrap_err();

    assert_eq!(load_error.kind, DesktopErrorKind::Stale);
    assert_eq!(run_error.kind, DesktopErrorKind::Stale);
}

#[test]
fn approval_dto_exposes_no_credential_envelope_or_project_authority() {
    let request = RequestEnvelope::project_current(
        RequestId::new("request-raw"),
        CallerId::new("caller-raw"),
        ProjectId::new("project-raw-authority"),
        IdempotencyKey::new("idempotency-raw"),
    )
    .authenticated(CallerCredential::new("credential-secret"));
    let pending = pending_approval_for_test(
        request,
        ApprovalChallenge {
            approval_id: ApprovalId::new("approval-raw"),
            expires_at_unix_ms: 42,
        },
    );

    let dto = current_dto_for_test(CurrentState::ApprovalRequired(pending));
    let json = serde_json::to_string(&dto).unwrap();

    assert!(matches!(dto, CurrentDto::ApprovalRequired { .. }));
    assert_eq!(
        serde_json::to_value(&dto).unwrap(),
        serde_json::json!({
            "status": "approval_required",
            "approval": serde_json::to_value(match &dto {
                CurrentDto::ApprovalRequired { approval, .. } => approval,
                _ => unreachable!(),
            }).unwrap(),
            "expiresAtMs": 42
        })
    );
    assert!(!json.contains("credential-secret"));
    assert!(!json.contains("project-raw-authority"));
    assert!(!json.contains("request-raw"));
    assert!(!json.contains("caller-raw"));
}

#[test]
fn current_access_and_evidence_conversions_remain_bounded_and_truthful() {
    let access = access_dto_for_test(AccessConfigState::Available(map_diagnostics_for_test(
        OperationTruth::Observed,
        &NetworkDiagnosticsResult {
            platform_roots_enabled: true,
            system_proxy_discovery_enabled: false,
            proxy_environment_presence: ConfigurationPresence::Configured,
            no_proxy_presence: ConfigurationPresence::NotConfigured,
            pac_state: PacState::DetectedUnsupported,
        },
    )));
    assert!(matches!(
        access,
        AccessConfigDto::Available {
            platform_roots_enabled: true,
            system_proxy_discovery_enabled: false,
            ..
        }
    ));

    let protocol_handle = EvidenceHandle::parse("evidence://test/result").unwrap();
    let body = "x".repeat(8 * 1024);
    let dto = evidence_dto_for_test(
        EvidenceHandleDto::new(),
        EvidencePreview {
            handle: protocol_handle,
            digest: "sha256:test".to_owned(),
            size_bytes: 8 * 1024,
            media_type: "text/plain".to_owned(),
            body: Some(body),
            truncated: true,
            truth: OperationTruth::Observed,
        },
    );
    assert!(dto.body.as_ref().unwrap().len() <= 4 * 1024);
    assert!(dto.truncated);
    assert_eq!(dto.truth, "observed");
}

#[test]
fn activity_dto_serializes_the_exact_frontend_ok_contract() {
    let dto = activity_dto_for_test(ObservatoryState::Available(ActivityResult {
        events: vec![ActivityEventSummary {
            sequence: 7,
            project_id: ProjectId::from("project-7"),
            caller_id: CallerId::from("gui"),
            action: "daemon.activity".to_owned(),
            decision: "allow".to_owned(),
            outcome: "success".to_owned(),
            occurred_at_ms: 123,
            project_root: Some("/work/project-7".to_owned()),
        }],
        truncated: false,
    }));

    assert_eq!(
        serde_json::to_value(dto).unwrap(),
        serde_json::json!({
            "status": "ok",
            "events": [{
                "sequence": 7,
                "projectId": "project-7",
                "callerId": "gui",
                "action": "daemon.activity",
                "decision": "allow",
                "outcome": "success",
                "occurredAtMs": 123,
                "projectRoot": "/work/project-7"
            }],
            "truncated": false
        })
    );
}

#[test]
fn callers_dto_serializes_the_exact_frontend_ok_contract() {
    let dto = callers_dto_for_test(ObservatoryState::Available(CallerListResult {
        callers: vec![
            CallerSummary {
                caller_id: CallerId::from("gui"),
                registered_at_ms: 123,
                revoked_at_ms: None,
                kind: Some("gui".to_owned()),
            },
            CallerSummary {
                caller_id: CallerId::from("cli"),
                registered_at_ms: 100,
                revoked_at_ms: Some(200),
                // A legacy row registered before `kind` existed.
                kind: None,
            },
        ],
    }));

    assert_eq!(
        serde_json::to_value(dto).unwrap(),
        serde_json::json!({
            "status": "ok",
            "callers": [
                { "callerId": "gui", "registeredAtMs": 123, "revokedAtMs": null, "kind": "gui" },
                { "callerId": "cli", "registeredAtMs": 100, "revokedAtMs": 200, "kind": null }
            ]
        })
    );
}

#[test]
fn observatory_denials_are_blocked_and_offline_reads_are_unavailable() {
    let blocked = activity_dto_for_test(ObservatoryState::Blocked {
        code: FailureCode::Forbidden,
        detail: "Policy denies daemon.activity.".to_owned(),
        recovery: None,
    });
    assert_eq!(
        serde_json::to_value(blocked).unwrap(),
        serde_json::json!({
            "status": "blocked",
            "failure": {
                "kind": "blocked",
                "code": "forbidden",
                "detail": "Policy denies daemon.activity.",
                "recovery": null
            }
        })
    );

    let unavailable = callers_dto_for_test(ObservatoryState::Unavailable {
        code: None,
        detail: "The PAM daemon is not running.".to_owned(),
        recovery: Some("Start the PAM daemon.".to_owned()),
    });
    assert_eq!(
        serde_json::to_value(unavailable).unwrap(),
        serde_json::json!({
            "status": "unavailable",
            "failure": {
                "kind": "unavailable",
                "code": null,
                "detail": "The PAM daemon is not running.",
                "recovery": "Start the PAM daemon."
            }
        })
    );
}

#[test]
fn model_status_dto_serializes_the_exact_frontend_ok_contract() {
    let dto = model_status_dto_for_test(ObservatoryState::Available(ModelStatusResult {
        loaded: Some(ModelSummary::new("qwen/qwen3-4b-instruct", 2_600_000_000).unwrap()),
        registered: vec![
            ModelSummary::new("qwen/qwen3-4b-instruct", 2_600_000_000).unwrap(),
            ModelSummary::new("vendor/name", 123).unwrap(),
        ],
        load_failure: None,
    }));

    assert_eq!(
        serde_json::to_value(dto).unwrap(),
        serde_json::json!({
            "status": "ok",
            "loaded": { "modelId": "qwen/qwen3-4b-instruct", "sizeBytes": 2_600_000_000_u64 },
            "registered": [
                { "modelId": "qwen/qwen3-4b-instruct", "sizeBytes": 2_600_000_000_u64 },
                { "modelId": "vendor/name", "sizeBytes": 123 }
            ],
            "loadFailure": null,
            "loading": false
        })
    );
}

#[test]
fn model_status_dto_reports_an_empty_surface_without_a_loaded_model() {
    let dto = model_status_dto_for_test(ObservatoryState::Available(ModelStatusResult {
        loaded: None,
        registered: Vec::new(),
        load_failure: None,
    }));

    assert_eq!(
        serde_json::to_value(dto).unwrap(),
        serde_json::json!({
            "status": "ok",
            "loaded": null,
            "registered": [],
            "loadFailure": null,
            "loading": false
        })
    );
}

/// A daemon serving without the model it was started with reports why: the
/// reason reaches the frontend on every status read, not just once as a
/// toast.
#[test]
fn model_status_dto_carries_the_daemon_reported_load_failure() {
    let dto = model_status_dto_for_test(ObservatoryState::Available(ModelStatusResult {
        loaded: None,
        registered: vec![ModelSummary::new("vendor/name", 123).unwrap()],
        load_failure: Some(
            "model load failed; the daemon will serve without a model: registered model does not \
             match the calibrated macOS runtime profile"
                .to_owned(),
        ),
    }));

    let value = serde_json::to_value(dto).unwrap();
    assert_eq!(value["status"], "ok");
    assert_eq!(value["loaded"], serde_json::Value::Null);
    assert!(
        value["loadFailure"]
            .as_str()
            .unwrap()
            .contains("calibrated macOS runtime profile")
    );
}

/// A model that does load clears the reason: the status carries no stale
/// failure once the runtime is up.
#[test]
fn model_status_dto_reports_no_load_failure_once_a_model_is_loaded() {
    let dto = model_status_dto_for_test(ObservatoryState::Available(ModelStatusResult {
        loaded: Some(ModelSummary::new("vendor/name", 123).unwrap()),
        registered: vec![ModelSummary::new("vendor/name", 123).unwrap()],
        load_failure: None,
    }));

    assert_eq!(
        serde_json::to_value(dto).unwrap()["loadFailure"],
        serde_json::Value::Null
    );
}

#[tokio::test]
async fn registered_model_catalog_reads_the_durable_store_directly() {
    let directory =
        std::env::temp_dir().join(format!("pam-gui-model-catalog-{}", uuid::Uuid::new_v4()));
    let state_path = directory.join("state.sqlite3");
    std::fs::create_dir_all(&directory).unwrap();
    let store = pam_store::Store::open(&state_path).unwrap();
    store
        .put_model(pam_model::RegisteredModel {
            key: pam_model::ModelKey::new("qwen", "seeded").unwrap(),
            path: directory.join("seeded.gguf"),
            digest: ContentDigest::from_sha256([7; 32]),
            size_bytes: 64,
            gguf: pam_model::GgufMetadata {
                version: 3,
                tensor_count: 17,
                metadata_kv_count: 29,
                architecture: None,
                model_name: None,
                license: None,
            },
            license: pam_model::LicenseSnapshot::new(
                "Apache-2.0",
                "https://example.test/license",
                ContentDigest::from_sha256([8; 32]),
            )
            .unwrap(),
            source: pam_model::ModelSource::Local,
            registered_at_ms: 5,
        })
        .await
        .unwrap();
    store.shutdown().await.unwrap();

    let catalog = registered_model_catalog_in_for_test(state_path)
        .await
        .expect("a readable store yields its catalog");
    assert_eq!(
        catalog,
        vec![ModelSummaryDto {
            model_id: "qwen/seeded".to_owned(),
            size_bytes: 64,
        }]
    );
    let _ = std::fs::remove_dir_all(&directory);
}

#[tokio::test]
async fn model_presets_lists_the_curated_catalog_judged_against_this_host() {
    let core = DesktopCore::new("/bounded/test");
    let dto = core
        .model_presets(daemon_fence(OperationId::new()))
        .await
        .unwrap();

    let ids: Vec<&str> = dto
        .presets
        .iter()
        .map(|preset| preset.id.as_str())
        .collect();
    assert_eq!(ids.len(), crate::model_presets::CATALOG.len());
    assert!(ids.contains(&"qwen3-coder-30b-q4ks"));
    assert!(ids.contains(&"devstral-small-2-24b-q4km"));

    for preset in &dto.presets {
        let source = crate::model_presets::find(&preset.id).unwrap();
        assert_eq!(preset.calibrated, source.calibrated(), "{}", preset.id);
        assert_eq!(
            preset.fits_host,
            dto.host_model_budget_bytes
                .is_none_or(|budget| preset.expected_size_bytes <= budget),
            "{}",
            preset.id
        );
    }
}

#[tokio::test]
async fn model_download_reports_an_unknown_preset_as_unavailable_data_not_an_error() {
    let core = DesktopCore::new("/bounded/test");
    let dto = core
        .model_download(
            daemon_fence(OperationId::new()),
            "does-not-exist".to_owned(),
        )
        .await
        .unwrap();

    assert!(matches!(dto, ModelDownloadDto::Unavailable { .. }));
}

#[tokio::test]
async fn model_import_status_defaults_to_idle() {
    let core = DesktopCore::new("/bounded/test");
    let dto = core
        .model_import_status(daemon_fence(OperationId::new()))
        .await
        .unwrap();

    assert_eq!(
        serde_json::to_value(dto).unwrap(),
        serde_json::json!({
            "status": "idle",
            "model": null,
            "stage": null,
            "hashedBytes": 0,
            "totalBytes": 0,
            "failure": null,
            "calibrated": false
        })
    );
}

#[tokio::test]
async fn model_download_cancel_without_a_run_is_unavailable_data_not_an_error() {
    let core = DesktopCore::new("/bounded/test");
    let dto = core
        .model_download_cancel(daemon_fence(OperationId::new()))
        .await
        .unwrap();

    assert!(matches!(dto, ModelDownloadDto::Unavailable { .. }));
}

#[tokio::test]
async fn model_download_status_defaults_to_idle() {
    let core = DesktopCore::new("/bounded/test");
    let dto = core
        .model_download_status(daemon_fence(OperationId::new()))
        .await
        .unwrap();

    assert_eq!(
        serde_json::to_value(dto).unwrap(),
        serde_json::json!({
            "status": "idle",
            "presetId": null,
            "receivedBytes": 0,
            "totalBytes": 0,
            "failure": null
        })
    );
}

#[test]
fn model_download_status_kinds_have_stable_exhaustive_wire_names() {
    use super::desktop::ModelDownloadStatusKindDto;
    assert_eq!(
        serde_json::to_value([
            ModelDownloadStatusKindDto::Idle,
            ModelDownloadStatusKindDto::Running,
            ModelDownloadStatusKindDto::Complete,
            ModelDownloadStatusKindDto::Failed,
            ModelDownloadStatusKindDto::Cancelled,
        ])
        .unwrap(),
        serde_json::json!(["idle", "running", "complete", "failed", "cancelled"])
    );
}

#[tokio::test]
async fn app_settings_reports_a_well_formed_default_snapshot() {
    // Reads the real user data and home directories (like the macOS host
    // memory probe test), but never writes: safe to run anywhere.
    let core = DesktopCore::new("/bounded/test");
    let dto: AppSettingsDto = core
        .app_settings(daemon_fence(OperationId::new()))
        .await
        .unwrap();

    // `models_dir_is_default` depends on whatever this machine's real PAM
    // Settings already persisted, so only the paths' shape is asserted here.
    assert!(std::path::Path::new(&dto.models_dir).is_absolute());
    assert!(std::path::Path::new(&dto.data_dir).is_absolute());
    assert!(std::path::Path::new(&dto.flows_dir).is_absolute());
    assert!(
        std::path::Path::new(&dto.flows_dir).ends_with(".pam/flows"),
        "the flows dir must name the daemon-global library layout"
    );
    assert!(std::path::Path::new(&dto.logs_dir).is_absolute());
    assert!(dto.logs_dir.ends_with("logs"));
}

#[tokio::test]
async fn reveal_path_rejects_a_path_that_is_not_a_known_settings_location() {
    let core = DesktopCore::new("/bounded/test");
    let error = core
        .reveal_path(
            daemon_fence(OperationId::new()),
            "/definitely/not/a/settings/path".to_owned(),
        )
        .await
        .unwrap_err();

    assert_eq!(error.kind, DesktopErrorKind::InvalidInput);
}

#[tokio::test]
async fn model_inspect_reports_identity_metadata_and_the_floor_verdict() {
    let core = DesktopCore::new("/bounded/test");
    let directory =
        std::env::temp_dir().join(format!("pam-gui-inspect-ok-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&directory).unwrap();
    let directory = directory.canonicalize().unwrap();
    let path = directory.join("model.gguf");
    std::fs::write(&path, crate::model_import_test::one_tensor_gguf()).unwrap();

    let dto = core
        .model_inspect(
            daemon_fence(OperationId::new()),
            path.to_str().unwrap().to_owned(),
        )
        .await
        .unwrap();

    match dto {
        ModelInspectDto::Ok {
            file_name,
            architecture,
            model_name,
            below_floor,
            ..
        } => {
            assert_eq!(file_name, "model.gguf");
            assert_eq!(architecture, None);
            assert_eq!(model_name, None);
            // The tiny fixture is far below PAM's recommended minimum.
            assert!(below_floor);
        }
        other => panic!("expected ModelInspectDto::Ok, got {other:?}"),
    }

    let _ = std::fs::remove_dir_all(&directory);
}

#[tokio::test]
async fn model_inspect_reports_a_non_gguf_file_as_unavailable_not_an_error() {
    let core = DesktopCore::new("/bounded/test");
    let directory =
        std::env::temp_dir().join(format!("pam-gui-inspect-bad-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&directory).unwrap();
    let directory = directory.canonicalize().unwrap();
    let path = directory.join("not-a-model.txt");
    std::fs::write(&path, b"not a gguf file").unwrap();

    let dto = core
        .model_inspect(
            daemon_fence(OperationId::new()),
            path.to_str().unwrap().to_owned(),
        )
        .await
        .unwrap();

    assert!(matches!(dto, ModelInspectDto::Unavailable { .. }));

    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn model_infer_dto_serializes_the_exact_frontend_ok_contract() {
    let dto = model_infer_dto_for_test(ObservatoryState::Available(
        ModelGenerationResult::new(
            "vendor/name",
            "Observed answer.",
            ModelFinishReason::Stop,
            ModelUsage {
                input_tokens: 1,
                sampled_output_tokens: 2,
                emitted_output_tokens: 2,
            },
        )
        .unwrap(),
    ));

    assert_eq!(
        serde_json::to_value(dto).unwrap(),
        serde_json::json!({
            "status": "ok",
            "model": "vendor/name",
            "text": "Observed answer.",
            "finishReason": "stop",
            "usage": { "inputTokens": 1, "sampledOutputTokens": 2, "emittedOutputTokens": 2 }
        })
    );
}

#[test]
fn model_infer_denials_are_blocked_with_recovery_and_transport_is_unavailable() {
    let blocked = model_infer_dto_for_test(ObservatoryState::Blocked {
        code: FailureCode::Forbidden,
        detail: "Policy denies model.infer.".to_owned(),
        recovery: Some("Approve model.infer for the GUI caller.".to_owned()),
    });
    assert_eq!(
        serde_json::to_value(blocked).unwrap(),
        serde_json::json!({
            "status": "blocked",
            "failure": {
                "kind": "blocked",
                "code": "forbidden",
                "detail": "Policy denies model.infer.",
                "recovery": "Approve model.infer for the GUI caller."
            }
        })
    );

    let unavailable = model_infer_dto_for_test(ObservatoryState::Unavailable {
        code: None,
        detail: "The PAM daemon is not running.".to_owned(),
        recovery: Some("Start the PAM daemon.".to_owned()),
    });
    assert_eq!(
        serde_json::to_value(unavailable).unwrap(),
        serde_json::json!({
            "status": "unavailable",
            "failure": {
                "kind": "unavailable",
                "code": null,
                "detail": "The PAM daemon is not running.",
                "recovery": "Start the PAM daemon."
            }
        })
    );
}

#[test]
fn model_output_token_requests_are_clamped_to_the_protocol_budget() {
    for (requested, expected) in [
        (None, 512),
        (Some(0), 512),
        (Some(1), 1),
        (Some(512), 512),
        (Some(MAX_MODEL_OUTPUT_TOKENS), MAX_MODEL_OUTPUT_TOKENS),
        (Some(MAX_MODEL_OUTPUT_TOKENS + 1), MAX_MODEL_OUTPUT_TOKENS),
        (Some(u32::MAX), MAX_MODEL_OUTPUT_TOKENS),
    ] {
        assert_eq!(clamp_model_output_tokens_for_test(requested), expected);
    }
}

fn connector_summary() -> ConnectorSummary {
    ConnectorSummary {
        connector_id: "github-actions".to_owned(),
        enabled: true,
        base_url: Some("https://api.github.com".to_owned()),
        credential_present: true,
        last_test_status: Some("passed".to_owned()),
        last_test_at_ms: Some(123),
    }
}

#[test]
fn connectors_dto_serializes_the_exact_frontend_ok_contract_without_secrets() {
    let dto = connectors_dto_for_test(ObservatoryState::Available(ConnectorListResult {
        connectors: vec![connector_summary()],
    }));

    let encoded = serde_json::to_string(&dto).unwrap();
    assert!(!encoded.contains("secret"));
    assert!(!encoded.contains("\"credential\""));
    assert_eq!(
        serde_json::to_value(dto).unwrap(),
        serde_json::json!({
            "status": "ok",
            "connectors": [{
                "connectorId": "github-actions",
                "enabled": true,
                "baseUrl": "https://api.github.com",
                "credentialPresent": true,
                "lastTestStatus": "passed",
                "lastTestAtMs": 123
            }]
        })
    );
}

#[test]
fn connector_configure_dto_serializes_the_exact_frontend_ok_contract() {
    let dto =
        connector_configure_dto_for_test(ObservatoryState::Available(ConnectorConfigureResult {
            connector: ConnectorSummary {
                base_url: None,
                last_test_status: None,
                last_test_at_ms: None,
                ..connector_summary()
            },
        }));

    assert_eq!(
        serde_json::to_value(dto).unwrap(),
        serde_json::json!({
            "status": "ok",
            "connector": {
                "connectorId": "github-actions",
                "enabled": true,
                "baseUrl": null,
                "credentialPresent": true,
                "lastTestStatus": null,
                "lastTestAtMs": null
            }
        })
    );
}

#[test]
fn connector_test_dto_serializes_the_exact_frontend_ok_contract() {
    let dto = connector_test_dto_for_test(ObservatoryState::Available(ConnectorTestResult {
        connector_id: "github-actions".to_owned(),
        status: ConnectorTestDisposition::Passed,
        detail: "Reached the connector endpoint.".to_owned(),
    }));

    assert_eq!(
        serde_json::to_value(dto).unwrap(),
        serde_json::json!({
            "status": "ok",
            "connectorId": "github-actions",
            "result": "passed",
            "detail": "Reached the connector endpoint."
        })
    );
}

#[test]
fn connector_denials_are_blocked_with_recovery_and_transport_is_unavailable() {
    let blocked = connector_configure_dto_for_test(ObservatoryState::Blocked {
        code: FailureCode::Forbidden,
        detail: "Policy denies connector.configure.".to_owned(),
        recovery: Some("Grant connector.configure to the GUI caller.".to_owned()),
    });
    assert_eq!(
        serde_json::to_value(blocked).unwrap(),
        serde_json::json!({
            "status": "blocked",
            "failure": {
                "kind": "blocked",
                "code": "forbidden",
                "detail": "Policy denies connector.configure.",
                "recovery": "Grant connector.configure to the GUI caller."
            }
        })
    );

    let unavailable = connector_test_dto_for_test(ObservatoryState::Unavailable {
        code: None,
        detail: "The PAM daemon is not running.".to_owned(),
        recovery: Some("Start the PAM daemon.".to_owned()),
    });
    assert_eq!(
        serde_json::to_value(unavailable).unwrap(),
        serde_json::json!({
            "status": "unavailable",
            "failure": {
                "kind": "unavailable",
                "code": null,
                "detail": "The PAM daemon is not running.",
                "recovery": "Start the PAM daemon."
            }
        })
    );
}

fn repo_flow_source() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.pam/flows/after-merge-checks.toml");
    std::fs::read_to_string(path).expect("the repo flow fixture must be readable")
}

#[test]
fn flow_graph_and_flow_compose_reach_a_fixpoint_on_the_repo_flow() {
    let FlowGraphDto::Ok { definition } = flow_graph_data_for_test(&repo_flow_source()) else {
        panic!("the repo flow must convert to a structured definition");
    };
    let encoded = serde_json::to_string(&definition).unwrap();
    let FlowComposeDto::Ok { source } = flow_compose_data_for_test(&encoded) else {
        panic!("the structured definition must compose back to TOML");
    };
    let FlowGraphDto::Ok {
        definition: reparsed,
    } = flow_graph_data_for_test(&source)
    else {
        panic!("the normalized TOML must convert back to a structured definition");
    };

    assert_eq!(definition, reparsed);
    assert_eq!(definition["schema_version"], serde_json::json!(2));
    assert_eq!(definition["id"], serde_json::json!("after-merge-checks"));
    assert_eq!(
        definition["steps"][0]["semantic"],
        serde_json::json!("observe")
    );
}

#[test]
fn flow_conversions_reject_invalid_documents_with_bounded_detail() {
    let graph = flow_graph_data_for_test("schema_version = 99");
    assert!(matches!(graph, FlowGraphDto::Invalid { .. }));

    let compose = flow_compose_data_for_test("{\"schema_version\":99}");
    assert!(matches!(compose, FlowComposeDto::Invalid { .. }));
}

#[test]
fn oversized_flow_documents_are_invalid_in_both_directions() {
    let oversized = "x".repeat(pam_flow::MAX_FLOW_DOCUMENT_BYTES + 1);

    assert!(matches!(
        flow_graph_data_for_test(&oversized),
        FlowGraphDto::Invalid { .. }
    ));
    assert!(matches!(
        flow_compose_data_for_test(&oversized),
        FlowComposeDto::Invalid { .. }
    ));
}

struct ScratchDir(std::path::PathBuf);

impl ScratchDir {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let sequence = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "pam-gui-desktop-{label}-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn flow_workspace_migrates_legacy_project_flows_into_the_global_library_once() {
    // OWNER DECISION (pam-flows-are-global): flow definitions are daemon-
    // global; a project's pre-existing `.pam/flows` files must be copied into
    // the global library exactly once, without touching the legacy files.
    let legacy = ScratchDir::new("legacy-project");
    let global = ScratchDir::new("global-library");
    let legacy_flows = legacy.path().join(".pam/flows");
    std::fs::create_dir_all(&legacy_flows).unwrap();
    let legacy_source = repo_flow_source();
    std::fs::write(legacy_flows.join("after-merge-checks.toml"), &legacy_source).unwrap();

    let project = ProjectHandle::new();
    let generation = GenerationId::new();
    let core = active_core_at_for_test(&project, generation.clone(), legacy.path());
    let fence = || CommandFence::new(project.clone(), generation.clone(), OperationId::new());

    let first = flow_workspace_at_for_test(&core, fence(), global.path().to_path_buf())
        .await
        .unwrap();
    assert_eq!(first.data.migrated, vec!["after-merge-checks".to_owned()]);
    assert_eq!(first.data.definitions.len(), 1);
    let migrated_path = global.path().join(".pam/flows/after-merge-checks.toml");
    assert!(migrated_path.is_file());
    // The legacy source is untouched, not moved.
    assert_eq!(
        std::fs::read_to_string(legacy_flows.join("after-merge-checks.toml")).unwrap(),
        legacy_source
    );

    let second = flow_workspace_at_for_test(&core, fence(), global.path().to_path_buf())
        .await
        .unwrap();
    assert!(
        second.data.migrated.is_empty(),
        "migration must be idempotent by definition id, not re-copy on every load"
    );
    assert_eq!(second.data.definitions.len(), 1);
}

#[tokio::test]
async fn flow_workspace_skips_migration_without_an_active_project() {
    let global = ScratchDir::new("global-library-daemon-only");
    let core = DesktopCore::new("/bounded/test");
    let fence = daemon_fence(OperationId::new());

    let loaded = flow_workspace_at_for_test(&core, fence, global.path().to_path_buf())
        .await
        .unwrap();

    assert!(loaded.data.migrated.is_empty());
    assert!(loaded.data.definitions.is_empty());
}

fn daemon_fence(operation: OperationId) -> CommandFence {
    CommandFence::new(
        ProjectHandle::parse("daemon").unwrap(),
        GenerationId::parse("daemon").unwrap(),
        operation,
    )
}

fn empty_catalog() -> CatalogDto {
    CatalogDto {
        projects: Vec::new(),
        warning: None,
    }
}

#[track_caller]
fn assert_daemon_replay_conflict<T>(result: DesktopResult<T>) {
    let error = result.err().expect("a replayed daemon operation must fail");
    assert_eq!(error.kind, DesktopErrorKind::Conflict);
    assert!(error.message.contains("daemon scope"));
}

#[test]
fn the_daemon_literal_is_reserved_to_project_handle_and_generation() {
    assert!(ProjectHandle::parse("daemon").is_ok());
    assert!(GenerationId::parse("daemon").is_ok());
    assert!(OperationId::parse("daemon").is_err());
    assert!(ProjectHandle::parse("daemons").is_err());
    assert!(GenerationId::parse("Daemon").is_err());
}

#[tokio::test]
async fn daemon_fence_authorizes_without_an_active_project_and_blocks_replay() {
    let core = DesktopCore::new("/bounded/test");
    let fence = daemon_fence(OperationId::new());

    reserve_daemon_for_test(&core, &fence).await.unwrap();

    let replay = reserve_daemon_for_test(&core, &fence).await.unwrap_err();
    assert_eq!(replay.kind, DesktopErrorKind::Conflict);

    let fresh = daemon_fence(OperationId::new());
    reserve_daemon_for_test(&core, &fresh).await.unwrap();
}

#[tokio::test]
async fn daemon_authority_requires_both_reserved_literals() {
    let core = DesktopCore::new("/bounded/test");
    let mixed_generation = CommandFence::new(
        ProjectHandle::parse("daemon").unwrap(),
        GenerationId::new(),
        OperationId::new(),
    );
    let mixed_handle = CommandFence::new(
        ProjectHandle::new(),
        GenerationId::parse("daemon").unwrap(),
        OperationId::new(),
    );

    for fence in [mixed_generation, mixed_handle] {
        let error = reserve_daemon_for_test(&core, &fence).await.unwrap_err();
        assert_eq!(error.kind, DesktopErrorKind::InvalidInput);
    }
}

#[tokio::test]
async fn daemon_scoped_commands_accept_the_daemon_authority_without_a_project() {
    // A replayed daemon operation conflicts before any credential or daemon
    // I/O, which proves each command routes through the daemon authority
    // (a project-fenced path would fail with "No project is active" instead).
    let core = DesktopCore::new("/bounded/test");
    let operation = OperationId::new();
    reserve_daemon_for_test(&core, &daemon_fence(operation.clone()))
        .await
        .unwrap();
    let fence = || daemon_fence(operation.clone());

    assert_daemon_replay_conflict(core.daemon_activity(fence(), None).await);
    assert_daemon_replay_conflict(core.caller_registry(fence()).await);
    assert_daemon_replay_conflict(core.model_status(fence()).await);
    assert_daemon_replay_conflict(
        core.model_infer(fence(), "vendor/name".to_owned(), Vec::new(), None)
            .await,
    );
    assert_daemon_replay_conflict(core.connector_registry(fence()).await);
    assert_daemon_replay_conflict(
        core.connector_configure(
            fence(),
            ConnectorConfigureParams {
                connector: "github-actions".to_owned(),
                enabled: None,
                base_url: None,
                credential: None,
            },
        )
        .await,
    );
    assert_daemon_replay_conflict(
        core.connector_test(fence(), "github-actions".to_owned())
            .await,
    );
    assert_daemon_replay_conflict(core.daemon_health(fence()).await);
    assert_daemon_replay_conflict(core.daemon_access_config(fence()).await);
    assert_daemon_replay_conflict(core.start_daemon(fence(), None).await);
    assert_daemon_replay_conflict(core.stop_daemon(fence()).await);
    assert_daemon_replay_conflict(core.model_presets(fence()).await);
    assert_daemon_replay_conflict(
        core.model_import(
            fence(),
            crate::model_import::ModelImportParams {
                model: "vendor/model".to_owned(),
                path: "/bounded/test/model.gguf".into(),
                license_id: "Apache-2.0".to_owned(),
                license_url: "https://example.test/license".to_owned(),
                license_notice_text: "notice".to_owned(),
                allow_small: true,
            },
        )
        .await,
    );
    assert_daemon_replay_conflict(core.model_import_status(fence()).await);
    // The replay conflict fires before any Hugging Face exchange, so this
    // stays network-free.
    assert_daemon_replay_conflict(
        core.model_license_discover(fence(), "vendor/model".to_owned())
            .await,
    );
    assert_daemon_replay_conflict(
        core.model_download(fence(), "qwen3-coder-30b-q4ks".to_owned())
            .await,
    );
    assert_daemon_replay_conflict(core.model_download_status(fence()).await);
    assert_daemon_replay_conflict(core.model_download_cancel(fence()).await);
    assert_daemon_replay_conflict(core.host_memory(fence()).await);
    assert_daemon_replay_conflict(core.app_settings(fence()).await);
    assert_daemon_replay_conflict(core.settings_update(fence(), None).await);
    assert_daemon_replay_conflict(core.logs_delete(fence()).await);
    assert_daemon_replay_conflict(
        core.reveal_path(fence(), "/not/a/settings/path".to_owned())
            .await,
    );
}

/// Inference can run for minutes, so it must never queue behind the global
/// command gate: a replayed daemon operation conflicts even while the gate is
/// deliberately held, proving fence authorization fired without acquiring it.
#[tokio::test]
async fn model_infer_authorizes_off_the_command_gate() {
    let core = DesktopCore::new("/bounded/test");
    let operation = OperationId::new();
    reserve_daemon_for_test(&core, &daemon_fence(operation.clone()))
        .await
        .unwrap();

    let gate = command_gate_for_test(&core);
    let _held = gate.lock().await;
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        core.model_infer(
            daemon_fence(operation),
            "vendor/name".to_owned(),
            Vec::new(),
            None,
        ),
    )
    .await
    .expect("model_infer must not wait on the command gate");
    assert_daemon_replay_conflict(result);
}

/// The observed access boundary is daemon truth, so its command must refuse a
/// project fence outright: no project identity can reach this read.
#[tokio::test]
async fn daemon_access_config_refuses_a_project_fence() {
    let core = DesktopCore::new("/bounded/test");
    let error = core
        .daemon_access_config(CommandFence::new(
            ProjectHandle::new(),
            GenerationId::new(),
            OperationId::new(),
        ))
        .await
        .unwrap_err();

    assert_eq!(error.kind, DesktopErrorKind::InvalidInput);
    assert!(error.message.contains("daemon authority fence"));
}

/// The daemon-scope read hands back the same access DTO the project snapshot
/// carries, so both paths render identically.
#[test]
fn daemon_access_config_reuses_the_snapshot_access_contract() {
    assert_eq!(
        serde_json::to_value(access_dto_for_test(AccessConfigState::Available(
            map_diagnostics_for_test(
                OperationTruth::Observed,
                &NetworkDiagnosticsResult {
                    platform_roots_enabled: true,
                    system_proxy_discovery_enabled: false,
                    proxy_environment_presence: ConfigurationPresence::NotConfigured,
                    no_proxy_presence: ConfigurationPresence::NotConfigured,
                    pac_state: PacState::NotDetected,
                },
            )
        )))
        .unwrap(),
        serde_json::json!({
            "status": "available",
            "truth": "observed",
            "platformRootsEnabled": true,
            "systemProxyDiscoveryEnabled": false,
            "proxyEnvironment": "not configured",
            "noProxy": "not configured",
            "pac": "not detected"
        })
    );
}

#[tokio::test]
async fn start_daemon_rejects_a_malformed_model_key_before_any_authority_io() {
    let core = DesktopCore::new("/bounded/test");
    for model in [
        "qwen3-no-vendor",
        "a//b",
        "",
        "vendor/",
        "a b/c",
        "--model=x/y",
    ] {
        let error = core
            .start_daemon(daemon_fence(OperationId::new()), Some(model.to_owned()))
            .await
            .unwrap_err();
        assert_eq!(
            error.kind,
            DesktopErrorKind::InvalidInput,
            "model {model:?}"
        );
    }
}

/// Writes an executable fake-daemon shell script into a fresh temp dir and
/// returns (script path, temp dir) so the caller can clean up.
#[cfg(unix)]
/// True while the process still exists (a zombie also answers `kill -0`, so
/// pair this with `try_wait` when liveness matters).
#[cfg(unix)]
fn alive(pid: u32) -> bool {
    std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("kill -0 {pid} 2>/dev/null"))
        .status()
        .unwrap()
        .success()
}

fn fake_daemon_script(name: &str, body: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let directory =
        std::env::temp_dir().join(format!("pam-gui-daemon-{name}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("fake-daemon");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    (path, directory)
}

/// Regression: a daemon that dies after spawn (e.g. its model load fails)
/// must not be reported as a successful start, and the error must point at
/// the captured stderr log. The health prober stays Offline so only process
/// liveness can end the wait — a dead child must surface as an error.
#[cfg(unix)]
#[tokio::test]
async fn a_daemon_that_exits_during_startup_is_not_a_successful_start() {
    let (executable, directory) = fake_daemon_script("early-exit", "exit 3");
    let mut child = std::process::Command::new(&executable).spawn().unwrap();

    let error = wait_for_daemon_serving_for_test(
        &mut child,
        None,
        std::time::Duration::from_secs(30),
        || async { HealthState::Offline },
        || async { false },
    )
    .await
    .unwrap_err();

    assert_eq!(error.kind, DesktopErrorKind::Unavailable);
    // A genuinely dead start is still collected: no zombie, nothing to track.
    assert!(child.try_wait().unwrap().is_some());
    assert!(error.message.contains("exited during startup"));
    assert!(
        error
            .recovery
            .as_deref()
            .unwrap()
            .contains("daemon-stderr.log")
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// The daemon now keeps serving when a requested model fails to load, so a
/// healthy answer alone must not pass startup verification.
///
/// Regression (#31): the verification used to kill that daemon, so a failed
/// model load left nothing running while the GUI reported otherwise. The
/// child must survive the verification for `start_daemon` to track it.
#[cfg(unix)]
#[tokio::test]
async fn a_serving_daemon_without_the_requested_model_is_reported_alive_and_not_reaped() {
    let (executable, directory) = fake_daemon_script("no-model", "sleep 30");
    let mut child = std::process::Command::new(&executable).spawn().unwrap();
    let pid = child.id();

    let outcome = wait_for_daemon_serving_for_test(
        &mut child,
        Some("vendor/model"),
        std::time::Duration::from_secs(30),
        || async {
            HealthState::Healthy {
                daemon_version: "test".to_owned(),
                queue_depth: 0,
            }
        },
        || async { false },
    )
    .await
    .unwrap();

    let DaemonStartup::ModelMissing(error) = outcome else {
        panic!("a serving daemon without its model must report the model failure")
    };
    // The process is still there and still ours to wait on: neither killed
    // nor reaped, so the message below is true when the GUI shows it.
    assert!(
        alive(pid),
        "daemon child {pid} was killed by the verification"
    );
    assert!(
        child.try_wait().unwrap().is_none(),
        "daemon child {pid} was collected by the verification"
    );
    let _ = child.kill();
    let _ = child.wait();
    assert_eq!(error.kind, DesktopErrorKind::Unavailable);
    assert!(error.message.contains("failed to load"));
    assert!(
        error
            .recovery
            .as_deref()
            .unwrap()
            .contains("daemon-stderr.log")
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// While the daemon loads a multi-GB model it is alive but deaf, which the
/// health probe reports as Degraded ("running but did not respond in time").
/// That must keep the startup poll going — erroring out here would fail
/// every healthy load seconds in.
#[cfg(unix)]
#[tokio::test]
async fn a_daemon_deaf_during_model_load_keeps_polling_until_it_serves() {
    let (executable, directory) = fake_daemon_script("deaf-then-healthy", "sleep 30");
    let mut child = std::process::Command::new(&executable).spawn().unwrap();
    let probes = std::sync::Arc::new(AtomicUsize::new(0));
    let probe_count = std::sync::Arc::clone(&probes);

    let result = wait_for_daemon_serving_for_test(
        &mut child,
        Some("vendor/model"),
        std::time::Duration::from_secs(30),
        move || {
            let probe_count = std::sync::Arc::clone(&probe_count);
            async move {
                if probe_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst) < 2 {
                    HealthState::Degraded {
                        detail: "PAM daemon (pid 1) is running but did not respond in time."
                            .to_owned(),
                        recovery: None,
                    }
                } else {
                    HealthState::Healthy {
                        daemon_version: "test".to_owned(),
                        queue_depth: 0,
                    }
                }
            }
        },
        || async { true },
    )
    .await;

    let _ = child.kill();
    assert!(matches!(result, Ok(DaemonStartup::Serving)));
    assert_eq!(probes.load(std::sync::atomic::Ordering::SeqCst), 3);
    let _ = std::fs::remove_dir_all(&directory);
}

/// Regression (#34): a 39 GB artifact needs a full digest revalidation plus
/// the Metal load, which outran the flat two-minute deadline — and the
/// deadline branch killed the daemon mid-load, leaving the user with a
/// silent UNREACHABLE. A process that is still running at the deadline is
/// still starting: it must be handed back alive for `start_daemon` to track.
#[cfg(unix)]
#[tokio::test]
async fn a_daemon_still_loading_at_the_deadline_is_reported_starting_and_not_killed() {
    let (executable, directory) = fake_daemon_script("still-loading", "sleep 30");
    let mut child = std::process::Command::new(&executable).spawn().unwrap();
    let pid = child.id();

    let outcome = wait_for_daemon_serving_for_test(
        &mut child,
        Some("vendor/model"),
        std::time::Duration::from_millis(1),
        // Mid-load the daemon is alive but deaf: bound, not yet accepting.
        || async {
            HealthState::Degraded {
                detail: "PAM daemon (pid 1) is running but did not respond in time.".to_owned(),
                recovery: None,
            }
        },
        || async { false },
    )
    .await
    .unwrap();

    let DaemonStartup::StillStarting(notice) = outcome else {
        panic!("a daemon still running at the deadline must be reported as still starting")
    };
    assert!(
        alive(pid),
        "daemon child {pid} was killed by the deadline branch"
    );
    assert!(
        child.try_wait().unwrap().is_none(),
        "daemon child {pid} was collected by the deadline branch"
    );
    let _ = child.kill();
    let _ = child.wait();
    assert!(notice.message.contains("still starting"));
    assert!(notice.recovery.as_deref().unwrap().contains("Leave PAM"));
    let _ = std::fs::remove_dir_all(&directory);
}

/// The startup budget is the artifact's work, not a flat guess: a base
/// allowance for binding plus one minute per 8 GiB to hash and map, capped
/// so no launch can hold the command gate indefinitely.
#[test]
fn the_startup_budget_scales_with_the_artifact_size_on_disk() {
    assert_eq!(
        startup_budget_for_bytes_for_test(None),
        std::time::Duration::from_mins(2)
    );
    // 17.5 GB: 2 minutes plus 3.
    assert_eq!(
        startup_budget_for_bytes_for_test(Some(17_500_000_000)),
        std::time::Duration::from_mins(5)
    );
    // 39.2 GB: 2 minutes plus 5.
    assert_eq!(
        startup_budget_for_bytes_for_test(Some(39_200_000_000)),
        std::time::Duration::from_mins(7)
    );
    assert_eq!(
        startup_budget_for_bytes_for_test(Some(u64::MAX)),
        std::time::Duration::from_mins(10)
    );
}

/// The daemon cannot answer while it loads, so the GUI infers the phase from
/// the child it spawned: unreachable plus a live process is a load in
/// flight, and the panel says so instead of showing a silent unreachable.
#[cfg(unix)]
#[tokio::test]
async fn an_unreachable_daemon_with_a_live_child_reports_its_model_as_loading() {
    let (executable, directory) = fake_daemon_script("loading-status", "sleep 30");
    let unreachable = || ModelStatusDto::Ok {
        loaded: None,
        registered: Vec::new(),
        load_failure: None,
        loading: false,
    };
    let mut live_slot = Some(std::process::Command::new(&executable).spawn().unwrap());

    let live = mark_model_loading_for_test(unreachable(), &mut live_slot);
    assert!(
        matches!(live, ModelStatusDto::Ok { loading: true, .. }),
        "a live spawned daemon that cannot answer is still loading"
    );

    let mut child = live_slot.take().unwrap();
    let _ = child.kill();
    let _ = child.wait();
    let exited = mark_model_loading_for_test(unreachable(), &mut Some(child));
    assert!(
        matches!(exited, ModelStatusDto::Ok { loading: false, .. }),
        "a daemon whose process is gone is not loading"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// Positive control: a serving daemon with the requested model reported
/// loaded passes startup verification immediately.
#[cfg(unix)]
#[tokio::test]
async fn a_serving_daemon_with_the_requested_model_loaded_starts_successfully() {
    let (executable, directory) = fake_daemon_script("loaded", "sleep 30");
    let mut child = std::process::Command::new(&executable).spawn().unwrap();

    let result = wait_for_daemon_serving_for_test(
        &mut child,
        Some("vendor/model"),
        std::time::Duration::from_secs(30),
        || async {
            HealthState::Healthy {
                daemon_version: "test".to_owned(),
                queue_depth: 0,
            }
        },
        || async { true },
    )
    .await;

    let _ = child.kill();
    assert!(matches!(result, Ok(DaemonStartup::Serving)));
    let _ = std::fs::remove_dir_all(&directory);
}

/// Regression: replacing the tracked daemon child (start-with-model and
/// restart flows) used to drop the previous handle without a wait, leaving a
/// zombie once the old process exited and an untracked live daemon holding
/// the ownership lock. The replaced child must be killed and collected.
#[cfg(unix)]
#[tokio::test]
async fn replacing_a_live_daemon_child_kills_and_collects_it() {
    let (executable, directory) = fake_daemon_script("replace", "sleep 30");
    let previous = std::process::Command::new(&executable).spawn().unwrap();
    let previous_pid = previous.id();
    let next = std::process::Command::new(&executable).spawn().unwrap();
    let mut slot = Some(previous);

    replace_daemon_child_for_test(&mut slot, next);

    // A killed-but-unreaped child would still answer kill -0 as a zombie.
    let gone = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("kill -0 {previous_pid} 2>/dev/null"))
        .status()
        .unwrap();
    assert!(
        !gone.success(),
        "replaced daemon child {previous_pid} is still alive or a zombie"
    );
    if let Some(mut child) = slot.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
    let _ = std::fs::remove_dir_all(&directory);
}

/// Regression: a daemon that exits right after the stop request must be
/// collected within the grace period — the old code kept the handle whenever
/// the child had not exited yet, and no later call would ever reap it.
#[cfg(unix)]
#[tokio::test]
async fn reap_daemon_child_collects_a_child_that_exits_within_the_grace_period() {
    let (executable, directory) = fake_daemon_script("graceful-exit", "sleep 0.2");
    let child = std::process::Command::new(&executable).spawn().unwrap();
    let pid = child.id();
    let mut slot = Some(child);

    reap_daemon_child_for_test(&mut slot);

    assert!(slot.is_none());
    let gone = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("kill -0 {pid} 2>/dev/null"))
        .status()
        .unwrap();
    assert!(
        !gone.success(),
        "daemon child {pid} is still alive or a zombie"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

#[tokio::test]
async fn project_scoped_commands_reject_the_daemon_authority() {
    let project = ProjectHandle::new();
    let generation = GenerationId::new();
    let core = active_core_for_test(&project, generation);

    let reserve = reserve_for_test(&core, &daemon_fence(OperationId::new()))
        .await
        .unwrap_err();
    let refresh = core
        .refresh(daemon_fence(OperationId::new()))
        .await
        .unwrap_err();

    assert_eq!(reserve.kind, DesktopErrorKind::Stale);
    assert_eq!(refresh.kind, DesktopErrorKind::Stale);
}

#[tokio::test]
async fn skill_commands_accept_the_daemon_authority_without_a_project() {
    // A replayed daemon operation conflicts before any filesystem or store
    // work, which proves each skill command routes through the daemon
    // authority instead of the active-project fence.
    let core = DesktopCore::new("/bounded/test");
    let operation = OperationId::new();
    reserve_daemon_for_test(&core, &daemon_fence(operation.clone()))
        .await
        .unwrap();
    let fence = || daemon_fence(operation.clone());

    assert_daemon_replay_conflict(core.skill_inventory(fence()).await);
    assert_daemon_replay_conflict(core.load_skill_audit(fence()).await);
    assert_daemon_replay_conflict(core.run_skill_audit(fence()).await);
    assert_daemon_replay_conflict(
        core.manage_skill_library(SkillLibraryRequest::Load {
            project_handle: ProjectHandle::parse("daemon").unwrap(),
            generation: GenerationId::parse("daemon").unwrap(),
            operation_id: operation.clone(),
        })
        .await,
    );
}

#[tokio::test]
async fn flow_commands_accept_the_daemon_authority_without_a_project() {
    // Flow definitions are daemon-global: a replayed daemon operation
    // conflicts before any catalog I/O, which proves each flow command routes
    // through the daemon authority instead of the active-project fence (a
    // project-fenced path would fail with "No project is active" instead).
    let core = DesktopCore::new("/bounded/test");
    let operation = OperationId::new();
    reserve_daemon_for_test(&core, &daemon_fence(operation.clone()))
        .await
        .unwrap();
    let fence = || daemon_fence(operation.clone());

    assert_daemon_replay_conflict(core.flow_workspace(fence()).await);
    assert_daemon_replay_conflict(core.open_flow(fence(), FlowDefinitionHandle::new()).await);
    assert_daemon_replay_conflict(
        core.validate_flow(fence(), FlowDocumentHandle::new(), String::new())
            .await,
    );
    assert_daemon_replay_conflict(
        core.save_flow(fence(), FlowDocumentHandle::new(), String::new())
            .await,
    );
    assert_daemon_replay_conflict(core.flow_graph(fence(), String::new()).await);
    assert_daemon_replay_conflict(core.flow_compose(fence(), String::new()).await);
}

#[tokio::test]
async fn flow_graph_and_flow_compose_succeed_under_the_daemon_authority() {
    // Visual mode edits the daemon-global library without any project: both
    // local transforms must complete under a fresh daemon fence.
    let core = DesktopCore::new("/bounded/test");

    let graph = core
        .flow_graph(daemon_fence(OperationId::new()), repo_flow_source())
        .await
        .unwrap();
    let FlowGraphDto::Ok { definition } = graph else {
        panic!("expected parsed definition: {graph:?}");
    };

    let encoded = serde_json::to_string(&definition).unwrap();
    let composed = core
        .flow_compose(daemon_fence(OperationId::new()), encoded)
        .await
        .unwrap();
    assert!(matches!(composed, FlowComposeDto::Ok { .. }));
}

#[tokio::test]
async fn daemon_scoped_library_actions_are_limited_to_the_global_manifest() {
    let core = DesktopCore::new("/bounded/test");
    let global = SkillLibraryRequest::Load {
        project_handle: ProjectHandle::parse("daemon").unwrap(),
        generation: GenerationId::parse("daemon").unwrap(),
        operation_id: OperationId::new(),
    };
    let per_project = SkillLibraryRequest::Enable {
        project_handle: ProjectHandle::parse("daemon").unwrap(),
        generation: GenerationId::parse("daemon").unwrap(),
        operation_id: OperationId::new(),
        entry_id: CanonicalEntryId::parse("review").unwrap(),
        version: ContentDigest::from_sha256([3; 32]),
        agent: SkillLibraryAgentDto::Claude,
    };

    manage_skill_library_without_io_for_test(&core, global, None)
        .await
        .unwrap();
    let rejected = manage_skill_library_without_io_for_test(&core, per_project, None)
        .await
        .unwrap_err();

    assert_eq!(rejected.kind, DesktopErrorKind::InvalidInput);
    assert!(rejected.message.contains("requires an active project"));
}

#[tokio::test]
async fn bootstrap_with_an_empty_catalog_reports_global_mode_without_error() {
    let core = DesktopCore::new("/bounded/test");

    let first = bootstrap_with_catalog_for_test(&core, OperationId::new(), empty_catalog())
        .await
        .unwrap();
    // The activation guard is released: a later bootstrap still works.
    let second = bootstrap_with_catalog_for_test(&core, OperationId::new(), empty_catalog())
        .await
        .unwrap();

    for dto in [first, second] {
        assert!(dto.snapshot.is_none());
        assert!(dto.catalog.projects.is_empty());
    }
    // No project was activated: project fences remain rejected.
    let fence = CommandFence::new(
        ProjectHandle::new(),
        GenerationId::new(),
        OperationId::new(),
    );
    let error = reserve_for_test(&core, &fence).await.unwrap_err();
    assert_eq!(error.kind, DesktopErrorKind::Stale);
    // The daemon authority still works in global mode.
    reserve_daemon_for_test(&core, &daemon_fence(OperationId::new()))
        .await
        .unwrap();
}

#[test]
fn bootstrap_dto_serializes_a_nullable_snapshot_in_the_frontend_contract() {
    let dto = BootstrapDto {
        catalog: empty_catalog(),
        snapshot: None,
    };

    assert_eq!(
        serde_json::to_value(dto).unwrap(),
        serde_json::json!({
            "catalog": { "projects": [], "warning": null },
            "snapshot": null
        })
    );
}

#[test]
fn daemon_health_reuses_the_standalone_health_contract() {
    assert_eq!(
        serde_json::to_value(HealthDto::Offline).unwrap(),
        serde_json::json!({ "status": "offline" })
    );
}

#[test]
fn global_daemon_start_prefers_the_user_home_directory() {
    let fallback = std::path::Path::new("/bounded/fallback");

    let cwd = daemon_start_cwd_for_test(fallback);

    match std::env::home_dir() {
        Some(home) => assert_eq!(cwd, home),
        None => assert_eq!(cwd, fallback),
    }
}
