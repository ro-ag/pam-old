use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::{
    app::{
        FlowResponseKind, audit_export, flow_recovery_cursor, flow_response_matches,
        flow_run_retry, migrate_legacy_flows, model_delete_weights, model_import,
        model_import_resource, model_unload, model_unregister, refuse_unconfirmed,
        render_model_catalog, render_model_sweep, render_model_verification, retention_prune,
        select_flow,
    },
    command::{ResetConfirmation, RetentionScopeArg},
    flow::FlowCatalog,
    render::EXIT_OPERATION_FAILED,
    request::RequestContext,
};

use pam_core::{CallerId, ContentDigest, IdempotencyKey, RequestId};
use pam_model::{
    GgufMetadata, LicenseSnapshot, ModelDescriptor, ModelKey, ModelSource, RegisteredModel,
};
use pam_platform::discover_project;
use pam_protocol::{
    CancellationDisposition, CancellationResult, DanglingRegistrationSummary, Failure, FailureCode,
    ModelSweepResult, ModelVerification, ModelVerifyResult, OperationTruth, OrphanWeightsSummary,
    ReplayResult, ResultBody, ResultPayload,
};
use uuid::Uuid;

#[test]
fn flow_run_binds_the_outer_project_root_from_a_subdirectory_while_the_catalog_stays_global() {
    // Flow definitions live in the daemon-global library now: the catalog a
    // run selects from is unrelated to the active project's directory. Only
    // the *run* is bound to a project (here, the outer root discovered from a
    // nested subdirectory) — a project-local `.pam/flows` file must not leak
    // into the global catalog used to resolve the definition.
    let root = std::env::temp_dir().join(format!("pam-cli-root-{}", Uuid::new_v4()));
    let global = std::env::temp_dir().join(format!("pam-cli-global-{}", Uuid::new_v4()));
    let legacy_flows = root.join(".pam/flows");
    let nested = root.join("subdirectory/nested");
    let global_flows = global.join(".pam/flows");
    fs::create_dir_all(&legacy_flows).unwrap();
    fs::create_dir_all(&nested).unwrap();
    fs::create_dir_all(&global_flows).unwrap();
    let project_id = Uuid::new_v4();
    fs::write(
        root.join(".pam/project.toml"),
        format!("version = 1\nproject_id = \"{project_id}\"\n"),
    )
    .unwrap();
    fs::write(
        legacy_flows.join("legacy-flow.toml"),
        super::flow_test::flow_source("legacy-flow", "Project-local legacy"),
    )
    .unwrap();
    fs::write(
        global_flows.join("outer-flow.toml"),
        super::flow_test::flow_source("outer-flow", "Outer flow"),
    )
    .unwrap();

    let project = discover_project(&nested).unwrap();
    assert_eq!(project.root(), fs::canonicalize(&root).unwrap());
    assert_eq!(project.id().as_str(), project_id.to_string());

    let catalog = FlowCatalog::load(&global).unwrap();
    assert_eq!(catalog.entries().len(), 1);
    let selected = select_flow(&catalog, "outer-flow").unwrap();
    assert_eq!(selected.definition.id(), "outer-flow");
    assert!(catalog.select("legacy-flow").is_err());
    assert!(selected.normalized.contains("id = \"outer-flow\""));

    let request = RequestContext::new_for_project(CallerId::from("cli-1"), &project, None)
        .flow_run(
            selected.source,
            Some(RequestId::from("outer-run")),
            None,
            project.root(),
        )
        .unwrap();
    assert_eq!(request.project_id, project.id().clone());
    let pam_protocol::RequestPayload::FlowRun { project_root, .. } = &request.payload else {
        panic!("expected flow run request")
    };
    assert_eq!(project_root.as_str(), project.root().to_str().unwrap());
    assert!(!format!("{request:?}").contains(project_root.as_str()));

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(global).unwrap();
}

#[test]
fn migrate_legacy_flows_copies_missing_definitions_into_the_global_library_once() {
    let legacy_root = std::env::temp_dir().join(format!("pam-cli-legacy-{}", Uuid::new_v4()));
    let global_root = std::env::temp_dir().join(format!("pam-cli-global-{}", Uuid::new_v4()));
    let legacy_flows = legacy_root.join(".pam/flows");
    fs::create_dir_all(&legacy_flows).unwrap();
    fs::create_dir_all(&global_root).unwrap();
    let source = super::flow_test::flow_source("outer-flow", "Outer flow");
    fs::write(legacy_flows.join("outer-flow.toml"), &source).unwrap();

    let migrated = migrate_legacy_flows(&global_root, &legacy_root);
    assert_eq!(migrated, vec!["outer-flow".to_owned()]);
    let migrated_path = global_root.join(".pam/flows/outer-flow.toml");
    assert_eq!(fs::read_to_string(&migrated_path).unwrap(), source);
    // The legacy source is untouched, not moved.
    assert_eq!(
        fs::read_to_string(legacy_flows.join("outer-flow.toml")).unwrap(),
        source
    );

    let second = migrate_legacy_flows(&global_root, &legacy_root);
    assert!(
        second.is_empty(),
        "migration must be idempotent by definition id, not re-copy on every call"
    );

    fs::remove_dir_all(legacy_root).unwrap();
    fs::remove_dir_all(global_root).unwrap();
}

fn import_descriptor(
    model_name: &str,
    filename: &str,
    size_bytes: u64,
    weights_byte: u8,
    license_id: &str,
    license_url: &str,
    license_byte: u8,
) -> ModelDescriptor {
    ModelDescriptor::new(
        ModelKey::new("vendor", model_name).unwrap(),
        filename,
        ContentDigest::from_sha256([weights_byte; 32]),
        size_bytes,
        LicenseSnapshot::new(
            license_id,
            license_url,
            ContentDigest::from_sha256([license_byte; 32]),
        )
        .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn administrative_storage_ranges_are_rejected_before_local_authorization() {
    assert_eq!(
        audit_export(Path::new("unused-audit-output"), u64::MAX, None, None, 1).await,
        EXIT_OPERATION_FAILED
    );
    assert_eq!(
        audit_export(Path::new("unused-audit-output"), 0, Some(u64::MAX), None, 1,).await,
        EXIT_OPERATION_FAILED
    );
    assert_eq!(
        retention_prune(RetentionScopeArg::Session, u64::MAX, None, 1).await,
        EXIT_OPERATION_FAILED
    );
}

#[tokio::test]
async fn model_import_requires_explicit_license_acceptance_before_path_or_store_access() {
    assert_eq!(
        model_import(
            ModelKey::new("vendor", "model").unwrap(),
            Path::new("/definitely/missing/model.gguf"),
            ContentDigest::from_sha256([1; 32]),
            24,
            "Apache-2.0".to_owned(),
            "https://example.test/LICENSE".to_owned(),
            ContentDigest::from_sha256([2; 32]),
            false,
            None,
        )
        .await,
        EXIT_OPERATION_FAILED
    );
}

#[tokio::test]
async fn model_unregister_requires_explicit_confirmation_before_any_daemon_exchange() {
    // No daemon runs in this test: reaching the exchange at all would fail
    // differently, so the refusal proves consent is checked first.
    assert_eq!(
        model_unregister(ModelKey::new("vendor", "model").unwrap(), false, None).await,
        EXIT_OPERATION_FAILED
    );
}

#[tokio::test]
async fn model_unload_requires_explicit_confirmation_before_any_daemon_exchange() {
    // No daemon runs in this test: reaching the exchange at all would fail
    // differently, so the refusal proves consent is checked first.
    assert_eq!(model_unload(false, None).await, EXIT_OPERATION_FAILED);
}

#[test]
fn model_catalog_lists_identity_size_digest_source_and_registration_time() {
    let catalog = vec![
        catalog_model("acme", "a-model", 24, ModelSource::Local),
        catalog_model(
            "byteshape",
            "qwen3.6-q4ks",
            16_492_334_496,
            ModelSource::https("https://models.example/qwen.gguf").unwrap(),
        ),
    ];

    let rendered = render_model_catalog(&catalog, false).unwrap();
    let lines = rendered.lines().collect::<Vec<_>>();

    assert_eq!(lines[0], "models=2 truth=observed");
    assert_eq!(
        lines[1],
        format!(
            "model=acme/a-model size_bytes=24 digest={} source=local registered_at_ms=42",
            ContentDigest::from_sha256([1; 32])
        )
    );
    assert!(lines[2].contains("model=byteshape/qwen3.6-q4ks"));
    assert!(lines[2].contains("source=https"));

    // An empty registry still reports the observation it made.
    assert_eq!(
        render_model_catalog(&[], false).unwrap(),
        "models=0 truth=observed\n"
    );

    let json = render_model_catalog(&catalog, true).unwrap();
    assert!(json.contains("\"schemaVersion\": 1"));
    assert!(json.contains("\"model\": \"acme/a-model\""));
    assert!(json.contains("\"sizeBytes\": 16492334496"));
    assert!(json.contains("\"source\": \"https\""));
    assert!(json.contains("\"registeredAtMs\": 42"));
}

fn catalog_model(
    vendor: &str,
    name: &str,
    size_bytes: u64,
    source: ModelSource,
) -> RegisteredModel {
    RegisteredModel {
        key: ModelKey::new(vendor, name).unwrap(),
        path: PathBuf::from("/models/weights.gguf"),
        digest: ContentDigest::from_sha256([1; 32]),
        size_bytes,
        gguf: GgufMetadata {
            version: 3,
            tensor_count: 17,
            metadata_kv_count: 29,
            architecture: None,
            model_name: None,
            license: None,
        },
        license: LicenseSnapshot::new(
            "Apache-2.0",
            "https://example.test/LICENSE",
            ContentDigest::from_sha256([2; 32]),
        )
        .unwrap(),
        source,
        registered_at_ms: 42,
    }
}

#[tokio::test]
async fn deleting_weights_requires_explicit_confirmation_before_any_daemon_exchange() {
    // No daemon runs in this test: reaching the exchange at all would fail
    // differently, so the refusal proves consent is checked first.
    assert_eq!(
        model_delete_weights(ModelKey::new("vendor", "model").unwrap(), false, None).await,
        EXIT_OPERATION_FAILED
    );
}

fn verification(model: &str, health: &str, detail: Option<&str>) -> ModelVerification {
    ModelVerification {
        model: model.to_owned(),
        path: format!("/models/{model}.gguf"),
        size_bytes: 4096,
        health: health.to_owned(),
        detail: detail.map(str::to_owned),
        source: "https".to_owned(),
        weights_deletable: health == "ok",
    }
}

#[test]
fn a_verification_report_names_each_models_health_and_the_sentence_behind_it() {
    let report = ModelVerifyResult {
        models: vec![
            verification("vendor/healthy", "ok", None),
            verification(
                "vendor/drifted",
                "digest_mismatch",
                Some("model SHA-256 did not match the expected digest"),
            ),
        ],
    };

    let rendered = render_model_verification(&report, &OperationTruth::Unresolved, false).unwrap();
    let lines = rendered.lines().collect::<Vec<_>>();

    assert_eq!(lines[0], "models=2 failed=1 truth=unresolved");
    assert_eq!(
        lines[1],
        "model=vendor/healthy health=ok size_bytes=4096 source=https weights_deletable=true path=/models/vendor/healthy.gguf"
    );
    assert_eq!(
        lines[2],
        "model=vendor/drifted health=digest_mismatch size_bytes=4096 source=https weights_deletable=false path=/models/vendor/drifted.gguf"
    );
    // A health label is never the only thing a reader gets.
    assert_eq!(
        lines[3],
        "  model SHA-256 did not match the expected digest"
    );

    // A whole catalog that still matches reports the verified truth it earned.
    let intact = ModelVerifyResult {
        models: vec![verification("vendor/healthy", "ok", None)],
    };
    assert!(
        render_model_verification(&intact, &OperationTruth::Verified, false)
            .unwrap()
            .starts_with("models=1 failed=0 truth=verified")
    );

    let json = render_model_verification(&report, &OperationTruth::Unresolved, true).unwrap();
    assert!(json.contains("\"schemaVersion\": 1"));
    assert!(json.contains("\"truth\": \"unresolved\""));
    assert!(json.contains("\"health\": \"digest_mismatch\""));
    assert!(json.contains("\"weightsDeletable\": false"));
}

#[test]
fn a_sweep_report_names_both_directions_with_sizes_and_the_directory_total() {
    let report = ModelSweepResult {
        models_dir: "/models".to_owned(),
        dangling: vec![DanglingRegistrationSummary {
            model: "vendor/gone".to_owned(),
            path: "/models/vendor/gone.gguf".to_owned(),
            size_bytes: 4096,
        }],
        orphans: vec![OrphanWeightsSummary {
            path: "/models/vendor/stray.gguf".to_owned(),
            size_bytes: 128,
        }],
        total_bytes: 8192,
    };

    let rendered = render_model_sweep(&report, false).unwrap();
    let lines = rendered.lines().collect::<Vec<_>>();

    assert_eq!(
        lines[0],
        "dangling=1 orphans=1 total_bytes=8192 truth=observed models_dir=/models"
    );
    assert_eq!(
        lines[1],
        "dangling model=vendor/gone size_bytes=4096 path=/models/vendor/gone.gguf"
    );
    assert_eq!(
        lines[2],
        "orphan size_bytes=128 path=/models/vendor/stray.gguf"
    );

    let json = render_model_sweep(&report, true).unwrap();
    assert!(json.contains("\"modelsDir\": \"/models\""));
    assert!(json.contains("\"totalBytes\": 8192"));
    assert!(json.contains("\"sizeBytes\": 128"));
}

#[test]
fn model_import_approval_resource_binds_every_immutable_import_effect_field() {
    let baseline = import_descriptor(
        "model",
        "weights.gguf",
        24,
        1,
        "Apache-2.0",
        "https://example.test/LICENSE",
        2,
    );
    let baseline_resource = model_import_resource(&baseline);
    assert!(
        baseline_resource
            .as_str()
            .contains("model:vendor/model:import-effect=sha256:")
    );
    for sensitive in ["weights.gguf", "Apache-2.0", "https://example.test/LICENSE"] {
        assert!(!baseline_resource.as_str().contains(sensitive));
    }

    for changed in [
        import_descriptor(
            "other",
            "weights.gguf",
            24,
            1,
            "Apache-2.0",
            "https://example.test/LICENSE",
            2,
        ),
        import_descriptor(
            "model",
            "other.gguf",
            24,
            1,
            "Apache-2.0",
            "https://example.test/LICENSE",
            2,
        ),
        import_descriptor(
            "model",
            "weights.gguf",
            25,
            1,
            "Apache-2.0",
            "https://example.test/LICENSE",
            2,
        ),
        import_descriptor(
            "model",
            "weights.gguf",
            24,
            3,
            "Apache-2.0",
            "https://example.test/LICENSE",
            2,
        ),
        import_descriptor(
            "model",
            "weights.gguf",
            24,
            1,
            "MIT",
            "https://example.test/LICENSE",
            2,
        ),
        import_descriptor(
            "model",
            "weights.gguf",
            24,
            1,
            "Apache-2.0",
            "https://example.test/OTHER-LICENSE",
            2,
        ),
        import_descriptor(
            "model",
            "weights.gguf",
            24,
            1,
            "Apache-2.0",
            "https://example.test/LICENSE",
            4,
        ),
    ] {
        assert_ne!(baseline_resource, model_import_resource(&changed));
    }
}

#[test]
fn flow_modes_accept_only_their_typed_payload_or_failure() {
    let failure = ResultBody::Failure(Failure {
        code: FailureCode::Internal,
        message: "bounded failure".to_owned(),
        recovery: None,
        approval: None,
    });
    let replay = ResultBody::Success {
        truth: OperationTruth::Observed,
        payload: ResultPayload::Replay(ReplayResult {
            target_request_id: RequestId::from("run-1"),
            through_sequence: 4,
            pending: true,
        }),
    };
    let cancellation = ResultBody::Success {
        truth: OperationTruth::Changed,
        payload: ResultPayload::Cancellation(CancellationResult {
            target_request_id: RequestId::from("run-1"),
            disposition: CancellationDisposition::Requested,
        }),
    };
    let terminal = terminal_flow_body();

    for expected in [
        FlowResponseKind::Run,
        FlowResponseKind::Wait,
        FlowResponseKind::Result,
        FlowResponseKind::Replay,
        FlowResponseKind::Cancellation,
    ] {
        assert!(flow_response_matches(&failure, expected));
    }
    assert!(flow_response_matches(&terminal, FlowResponseKind::Run));
    assert!(flow_response_matches(&terminal, FlowResponseKind::Wait));
    assert!(flow_response_matches(&terminal, FlowResponseKind::Result));
    assert!(!flow_response_matches(&terminal, FlowResponseKind::Replay));
    assert!(!flow_response_matches(
        &terminal,
        FlowResponseKind::Cancellation
    ));
    assert!(flow_response_matches(&replay, FlowResponseKind::Replay));
    assert!(!flow_response_matches(&replay, FlowResponseKind::Run));
    assert!(flow_response_matches(
        &cancellation,
        FlowResponseKind::Cancellation
    ));
    assert!(!flow_response_matches(
        &cancellation,
        FlowResponseKind::Replay
    ));
}

#[test]
fn flow_recovery_cursor_never_confuses_observer_events_with_target_events() {
    assert_eq!(flow_recovery_cursor(FlowResponseKind::Cancellation, 42), 0);
    assert_eq!(flow_recovery_cursor(FlowResponseKind::Result, 42), 0);
    assert_eq!(flow_recovery_cursor(FlowResponseKind::Run, 42), 42);
    assert_eq!(flow_recovery_cursor(FlowResponseKind::Wait, 42), 42);
    assert_eq!(flow_recovery_cursor(FlowResponseKind::Replay, 42), 42);
}

#[test]
fn flow_run_recovery_uses_the_canonical_id_and_exact_durable_identity() {
    assert_eq!(
        flow_run_retry(
            "flow-alpha",
            &RequestId::from("stable-run"),
            &IdempotencyKey::from("stable-key"),
        ),
        "pam flow run flow-alpha --run-id stable-run --idempotency-key stable-key"
    );
}

fn terminal_flow_body() -> ResultBody {
    let definition = pam_flow::FlowDefinition::parse_toml(&super::flow_test::flow_source(
        "response-flow",
        "Response flow",
    ))
    .unwrap();
    let mut run =
        pam_flow::FlowRun::start(pam_flow::RunId::parse("response-run").unwrap(), definition)
            .unwrap();
    let update = run.cancel().unwrap();
    let pam_flow::RunDecision::Terminal { result } = update.decision() else {
        panic!("cancel before execution must be terminal")
    };
    ResultBody::Success {
        truth: OperationTruth::Unresolved,
        payload: ResultPayload::FlowRun(result.clone()),
    }
}

#[test]
fn a_reset_that_would_change_state_refuses_without_an_explicit_confirmation() {
    let confirmation = |dry_run: bool, yes: bool| ResetConfirmation {
        dry_run,
        yes,
        approval_id: None,
    };
    assert_eq!(
        refuse_unconfirmed(&confirmation(false, false)),
        Some(EXIT_OPERATION_FAILED)
    );
    // A forecast changes nothing, so it never needs a confirmation.
    assert_eq!(refuse_unconfirmed(&confirmation(true, false)), None);
    assert_eq!(refuse_unconfirmed(&confirmation(false, true)), None);
}
