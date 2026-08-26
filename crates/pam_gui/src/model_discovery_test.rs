use crate::model_discovery::{DiscoveredLicense, select_license};

fn body(results: &serde_json::Value) -> String {
    results.to_string()
}

#[test]
fn selects_the_matching_repo_with_a_concrete_license_tag() {
    let body = body(&serde_json::json!([
        { "id": "someone-else/unrelated", "tags": ["license:mit"] },
        { "id": "Qwen/Qwen3-Coder-30B-A3B-Instruct", "tags": ["transformers", "license:apache-2.0"] },
    ]));

    let discovered = select_license("qwen3-coder-30b-a3b-instruct", &body).unwrap();
    assert_eq!(
        discovered,
        DiscoveredLicense {
            repo_id: "Qwen/Qwen3-Coder-30B-A3B-Instruct".to_owned(),
            license_id: "apache-2.0".to_owned(),
        }
    );
}

#[test]
fn a_query_that_contains_the_repo_id_also_matches() {
    // GGUF general.name is often longer than the repo id (quant suffixes).
    let body = body(&serde_json::json!([
        { "id": "acme/tiny-model", "tags": ["license:mit"] },
    ]));

    let discovered = select_license("acme/tiny-model-q4_k_s-gguf", &body).unwrap();
    assert_eq!(discovered.license_id, "mit");
}

#[test]
fn placeholder_and_missing_licenses_are_skipped_for_a_later_concrete_match() {
    let body = body(&serde_json::json!([
        { "id": "acme/model", "tags": ["license:other"] },
        { "id": "acme/model-gguf", "tags": [] },
        { "id": "acme/model-instruct", "tags": ["license:llama3"] },
    ]));

    let discovered = select_license("acme/model", &body).unwrap();
    assert_eq!(discovered.repo_id, "acme/model-instruct");
    assert_eq!(discovered.license_id, "llama3");
}

#[test]
fn no_match_and_malformed_bodies_are_bounded_failures_with_recovery() {
    let empty = select_license("acme/model", "[]").unwrap_err();
    assert!(empty.detail.contains("No matching"));
    assert!(empty.recovery.as_deref().unwrap().contains("Advanced"));

    let malformed = select_license("acme/model", "not json").unwrap_err();
    assert!(malformed.detail.contains("unexpected answer"));
    // Never echo the body back.
    assert!(!malformed.detail.contains("not json"));
}

#[test]
fn an_unrelated_result_never_matches() {
    let body = body(&serde_json::json!([
        { "id": "someone/completely-different", "tags": ["license:mit"] },
    ]));

    assert!(select_license("acme/model", &body).is_err());
}
