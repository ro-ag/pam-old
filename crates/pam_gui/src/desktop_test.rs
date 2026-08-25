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
    current::{CurrentState, EvidencePreview, pending_approval_for_test},
    desktop::{
        AccessConfigDto, ApprovalDecisionDispositionDto, BootstrapDto, CatalogDto, CommandFence,
        ConnectorConfigureParams, CurrentDto, DesktopCore, DesktopErrorKind, DesktopResult,
        EvidenceHandleDto, FailureDto, FailureKindDto, FlowComposeDto, FlowGraphDto, GenerationId,
        HealthDto, ModelDownloadDto, OperationId, ProjectHandle, TimelineKindDto,
        access_dto_for_test, active_core_for_test, activity_dto_for_test,
        approval_current_for_test, approval_failure_retains_handle_for_test,
        bootstrap_with_catalog_for_test, bounded_detail_for_test, callers_dto_for_test,
        clamp_model_output_tokens_for_test, connector_configure_dto_for_test,
        connector_test_dto_for_test, connectors_dto_for_test, current_dto_for_test,
        daemon_start_cwd_for_test, evidence_dto_for_test, failure_kind_for_test,
        flow_compose_data_for_test, flow_graph_data_for_test, gui_registration_current_for_test,
        manage_skill_library_without_io_for_test, model_infer_dto_for_test,
        model_status_dto_for_test, post_save_reload_error_for_test, registration_contract_for_test,
        registration_failure_detail, reserve_daemon_for_test, reserve_for_test,
        switch_authority_for_test,
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
                "occurredAtMs": 123
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
            },
            CallerSummary {
                caller_id: CallerId::from("cli"),
                registered_at_ms: 100,
                revoked_at_ms: Some(200),
            },
        ],
    }));

    assert_eq!(
        serde_json::to_value(dto).unwrap(),
        serde_json::json!({
            "status": "ok",
            "callers": [
                { "callerId": "gui", "registeredAtMs": 123, "revokedAtMs": null },
                { "callerId": "cli", "registeredAtMs": 100, "revokedAtMs": 200 }
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
    }));

    assert_eq!(
        serde_json::to_value(dto).unwrap(),
        serde_json::json!({
            "status": "ok",
            "loaded": { "modelId": "qwen/qwen3-4b-instruct", "sizeBytes": 2_600_000_000_u64 },
            "registered": [
                { "modelId": "qwen/qwen3-4b-instruct", "sizeBytes": 2_600_000_000_u64 },
                { "modelId": "vendor/name", "sizeBytes": 123 }
            ]
        })
    );
}

#[test]
fn model_status_dto_reports_an_empty_surface_without_a_loaded_model() {
    let dto = model_status_dto_for_test(ObservatoryState::Available(ModelStatusResult {
        loaded: None,
        registered: Vec::new(),
    }));

    assert_eq!(
        serde_json::to_value(dto).unwrap(),
        serde_json::json!({ "status": "ok", "loaded": null, "registered": [] })
    );
}

#[tokio::test]
async fn model_presets_lists_exactly_the_curated_catalog() {
    let core = DesktopCore::new("/bounded/test");
    let dto = core
        .model_presets(daemon_fence(OperationId::new()))
        .await
        .unwrap();

    assert_eq!(dto.presets.len(), 3);
    let mut ids: Vec<&str> = dto
        .presets
        .iter()
        .map(|preset| preset.id.as_str())
        .collect();
    ids.sort_unstable();
    assert_eq!(
        ids,
        [
            "qwen3-coder-30b-q4km",
            "qwen3-coder-30b-q4ks",
            "qwen3-coder-30b-q6k"
        ]
    );
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
    assert_daemon_replay_conflict(core.start_daemon(fence(), None).await);
    assert_daemon_replay_conflict(core.stop_daemon(fence()).await);
    assert_daemon_replay_conflict(core.model_presets(fence()).await);
    assert_daemon_replay_conflict(
        core.model_download(fence(), "qwen3-coder-30b-q4ks".to_owned())
            .await,
    );
    assert_daemon_replay_conflict(core.model_download_status(fence()).await);
    assert_daemon_replay_conflict(core.host_memory(fence()).await);
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

#[tokio::test]
async fn project_scoped_commands_reject_the_daemon_authority() {
    let project = ProjectHandle::new();
    let generation = GenerationId::new();
    let core = active_core_for_test(&project, generation);

    let reserve = reserve_for_test(&core, &daemon_fence(OperationId::new()))
        .await
        .unwrap_err();
    let compose = core
        .flow_compose(daemon_fence(OperationId::new()), String::new())
        .await
        .unwrap_err();

    assert_eq!(reserve.kind, DesktopErrorKind::Stale);
    assert_eq!(compose.kind, DesktopErrorKind::Stale);
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
