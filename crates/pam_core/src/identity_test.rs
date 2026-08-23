use super::{ApprovalId, CallerId, GrantId, IdempotencyKey, ProjectId, RequestId};
use crate::identity::{CallerCredential, MAX_CALLER_CREDENTIAL_LENGTH};

#[test]
fn identifiers_preserve_their_text_values() {
    assert_eq!(CallerId::from("cli").as_str(), "cli");
    assert_eq!(ApprovalId::from("approval").as_str(), "approval");
    assert_eq!(GrantId::from("grant").as_str(), "grant");
    assert_eq!(IdempotencyKey::from("status-1").as_str(), "status-1");
    assert_eq!(ProjectId::from("project").as_str(), "project");
    assert_eq!(RequestId::from("request").as_str(), "request");
}

#[test]
fn daemon_scope_uses_the_reserved_wire_literal() {
    let scope = ProjectId::daemon_scope();

    assert_eq!(scope.as_str(), "daemon");
    assert!(scope.is_daemon_scope());
    assert!(!ProjectId::from("project").is_daemon_scope());
}

#[test]
fn daemon_scope_construction_is_canonical() {
    // Any construction of the reserved literal is the same identity, so a
    // spoofed "daemon" project id cannot be distinguished from the scope and
    // is subject to the same project-scoped rejections.
    assert_eq!(ProjectId::from("daemon"), ProjectId::daemon_scope());
    assert_eq!(
        ProjectId::new(String::from("daemon")),
        ProjectId::daemon_scope()
    );
}

#[test]
fn caller_credential_debug_output_is_redacted() {
    let credential = CallerCredential::new("credential-secret");

    assert_eq!(format!("{credential:?}"), "[REDACTED]");
}

#[test]
fn caller_credentials_support_equality_and_explicit_secret_access() {
    let credential = CallerCredential::new(String::from("credential-secret"));

    assert_eq!(credential, CallerCredential::new("credential-secret"));
    assert_eq!(credential.expose_secret(), "credential-secret");
}

#[test]
fn caller_credential_validation_enforces_byte_length_boundaries() {
    assert!(!CallerCredential::new("").is_valid());
    assert!(CallerCredential::new("x").is_valid());
    assert!(CallerCredential::new("x".repeat(MAX_CALLER_CREDENTIAL_LENGTH)).is_valid());
    assert!(!CallerCredential::new("x".repeat(MAX_CALLER_CREDENTIAL_LENGTH + 1)).is_valid());

    let multibyte_credential = "é".repeat(MAX_CALLER_CREDENTIAL_LENGTH / 2 + 1);
    assert!(!CallerCredential::new(multibyte_credential).is_valid());
}
