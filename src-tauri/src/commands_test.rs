use serde_json::json;

use pam_gui::SkillLibraryRequest;

use crate::commands::{
    ActivateProjectRequest, ActivityRequest, ApprovalRequest, BootstrapRequest, FencedRequest,
    FlowComposeRequest, FlowGraphRequest, ModelInferRequest,
};

const PROJECT_HANDLE: &str = "88d408ec-796b-4f56-b34c-f2a8d25f9128";
const GENERATION: &str = "c608f63b-cd23-45af-87ed-5a13bf559154";
const OPERATION_ID: &str = "df002b98-c404-40db-9dc6-57382e686612";

#[test]
fn command_requests_reject_unknown_fields() {
    let request = json!({
        "projectHandle": PROJECT_HANDLE,
        "generation": GENERATION,
        "operationId": OPERATION_ID,
        "unexpected": "ambient authority"
    });

    assert!(serde_json::from_value::<FencedRequest>(request).is_err());
}

#[test]
fn fenced_request_accepts_only_canonical_uuid_fields() {
    let request = json!({
        "projectHandle": PROJECT_HANDLE,
        "generation": GENERATION,
        "operationId": OPERATION_ID
    });
    let noncanonical = json!({
        "projectHandle": PROJECT_HANDLE,
        "generation": GENERATION,
        "operationId": OPERATION_ID.to_uppercase()
    });

    assert!(serde_json::from_value::<FencedRequest>(request).is_ok());
    assert!(serde_json::from_value::<FencedRequest>(noncanonical).is_err());
}

#[test]
fn activation_requires_a_canonical_operation_uuid() {
    let malformed = json!({
        "projectHandle": PROJECT_HANDLE,
        "operationId": "not-an-operation-uuid"
    });
    let noncanonical = json!({
        "projectHandle": PROJECT_HANDLE,
        "operationId": OPERATION_ID.to_uppercase()
    });

    assert!(serde_json::from_value::<ActivateProjectRequest>(malformed).is_err());
    assert!(serde_json::from_value::<ActivateProjectRequest>(noncanonical).is_err());
}

#[test]
fn bootstrap_requires_an_operation_uuid_and_rejects_ambient_fields() {
    let missing = json!({});
    let ambient = json!({
        "operationId": OPERATION_ID,
        "projectRoot": "/tmp/untrusted"
    });

    assert!(serde_json::from_value::<BootstrapRequest>(missing).is_err());
    assert!(serde_json::from_value::<BootstrapRequest>(ambient).is_err());
}

#[test]
fn approval_request_requires_the_complete_fence() {
    let missing_generation = json!({
        "projectHandle": PROJECT_HANDLE,
        "operationId": OPERATION_ID,
        "approvalHandle": "c4cf012a-ee69-4ecb-a7a8-3516ed521a07",
        "decision": "approve"
    });

    assert!(serde_json::from_value::<ApprovalRequest>(missing_generation).is_err());
}

#[test]
fn inventory_request_rejects_paths_and_accepts_only_the_fence() {
    let valid = json!({
        "projectHandle": PROJECT_HANDLE,
        "generation": GENERATION,
        "operationId": OPERATION_ID
    });
    let ambient_path = json!({
        "projectHandle": PROJECT_HANDLE,
        "generation": GENERATION,
        "operationId": OPERATION_ID,
        "projectRoot": "/tmp/untrusted"
    });

    assert!(serde_json::from_value::<FencedRequest>(valid).is_ok());
    assert!(serde_json::from_value::<FencedRequest>(ambient_path).is_err());
}

#[test]
fn audit_requests_accept_only_the_canonical_fence_contract() {
    let valid = json!({
        "projectHandle": PROJECT_HANDLE,
        "generation": GENERATION,
        "operationId": OPERATION_ID
    });
    let ambient_evaluator = json!({
        "projectHandle": PROJECT_HANDLE,
        "generation": GENERATION,
        "operationId": OPERATION_ID,
        "path": "/tmp/evaluator",
        "evaluatorConfig": { "deadlineMs": 1 }
    });

    assert!(serde_json::from_value::<FencedRequest>(valid).is_ok());
    assert!(serde_json::from_value::<FencedRequest>(ambient_evaluator).is_err());
}

#[test]
fn skill_library_request_is_one_strict_action_without_caller_roots_or_project_identity() {
    let valid = json!({
        "action": "load",
        "projectHandle": PROJECT_HANDLE,
        "generation": GENERATION,
        "operationId": OPERATION_ID
    });
    let ambient = json!({
        "action": "load",
        "projectHandle": PROJECT_HANDLE,
        "generation": GENERATION,
        "operationId": OPERATION_ID,
        "projectId": "guessed-project",
        "root": "/tmp/untrusted"
    });

    assert!(serde_json::from_value::<SkillLibraryRequest>(valid).is_ok());
    assert!(serde_json::from_value::<SkillLibraryRequest>(ambient).is_err());
}

#[test]
fn activity_request_accepts_an_optional_limit_and_rejects_unknown_fields() {
    let without_limit = json!({
        "projectHandle": PROJECT_HANDLE,
        "generation": GENERATION,
        "operationId": OPERATION_ID
    });
    let with_limit = json!({
        "projectHandle": PROJECT_HANDLE,
        "generation": GENERATION,
        "operationId": OPERATION_ID,
        "limit": 25
    });
    let unknown = json!({
        "projectHandle": PROJECT_HANDLE,
        "generation": GENERATION,
        "operationId": OPERATION_ID,
        "callerId": "ambient authority"
    });

    assert!(serde_json::from_value::<ActivityRequest>(without_limit).is_ok());
    assert!(serde_json::from_value::<ActivityRequest>(with_limit).is_ok());
    assert!(serde_json::from_value::<ActivityRequest>(unknown).is_err());
}

#[test]
fn model_infer_request_accepts_the_fence_conversation_and_optional_token_budget() {
    let with_budget = json!({
        "projectHandle": PROJECT_HANDLE,
        "generation": GENERATION,
        "operationId": OPERATION_ID,
        "model": "vendor/name",
        "messages": [
            { "role": "system", "content": "Answer briefly." },
            { "role": "user", "content": "What changed?" }
        ],
        "maxOutputTokens": 256
    });
    let without_budget = json!({
        "projectHandle": PROJECT_HANDLE,
        "generation": GENERATION,
        "operationId": OPERATION_ID,
        "model": "vendor/name",
        "messages": [{ "role": "user", "content": "What changed?" }]
    });

    assert!(serde_json::from_value::<ModelInferRequest>(with_budget).is_ok());
    assert!(serde_json::from_value::<ModelInferRequest>(without_budget).is_ok());
}

#[test]
fn model_infer_request_rejects_unknown_fields_and_unknown_roles() {
    let ambient = json!({
        "projectHandle": PROJECT_HANDLE,
        "generation": GENERATION,
        "operationId": OPERATION_ID,
        "model": "vendor/name",
        "messages": [{ "role": "user", "content": "What changed?" }],
        "modelPath": "/tmp/untrusted.gguf"
    });
    let unknown_role = json!({
        "projectHandle": PROJECT_HANDLE,
        "generation": GENERATION,
        "operationId": OPERATION_ID,
        "model": "vendor/name",
        "messages": [{ "role": "tool", "content": "ambient authority" }]
    });
    let ambient_message_field = json!({
        "projectHandle": PROJECT_HANDLE,
        "generation": GENERATION,
        "operationId": OPERATION_ID,
        "model": "vendor/name",
        "messages": [{ "role": "user", "content": "What changed?", "path": "/tmp" }]
    });

    assert!(serde_json::from_value::<ModelInferRequest>(ambient).is_err());
    assert!(serde_json::from_value::<ModelInferRequest>(unknown_role).is_err());
    assert!(serde_json::from_value::<ModelInferRequest>(ambient_message_field).is_err());
}

#[test]
fn flow_graph_request_accepts_only_the_fence_and_source_text() {
    let valid = json!({
        "projectHandle": PROJECT_HANDLE,
        "generation": GENERATION,
        "operationId": OPERATION_ID,
        "source": "schema_version = 2"
    });
    let ambient_path = json!({
        "projectHandle": PROJECT_HANDLE,
        "generation": GENERATION,
        "operationId": OPERATION_ID,
        "source": "schema_version = 2",
        "path": "/tmp/untrusted.toml"
    });

    assert!(serde_json::from_value::<FlowGraphRequest>(valid).is_ok());
    assert!(serde_json::from_value::<FlowGraphRequest>(ambient_path).is_err());
}

#[test]
fn flow_compose_request_accepts_only_the_fence_and_definition_text() {
    let valid = json!({
        "projectHandle": PROJECT_HANDLE,
        "generation": GENERATION,
        "operationId": OPERATION_ID,
        "definition": "{}"
    });
    let missing_definition = json!({
        "projectHandle": PROJECT_HANDLE,
        "generation": GENERATION,
        "operationId": OPERATION_ID
    });

    assert!(serde_json::from_value::<FlowComposeRequest>(valid).is_ok());
    assert!(serde_json::from_value::<FlowComposeRequest>(missing_definition).is_err());
}
