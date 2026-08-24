use std::path::PathBuf;

use pam_core::ProjectId;
use pam_protocol::StatusResult;

use super::control_center::{HealthState, ProjectEntry, classify_status, merge_test_projects};

fn project(name: &str, path: &str, id: Option<&str>) -> ProjectEntry {
    ProjectEntry {
        name: name.to_owned(),
        root: PathBuf::from(path),
        id: id.map(ProjectId::new),
    }
}

#[test]
fn healthy_status_remains_healthy_while_work_is_queued() {
    let state = classify_status(StatusResult {
        ready: true,
        healthy: true,
        daemon_version: "0.1.0".to_owned(),
        protocol_version: 6,
        queue_depth: 3,
    });

    assert_eq!(
        state,
        HealthState::Healthy {
            daemon_version: "0.1.0".to_owned(),
            queue_depth: 3,
        }
    );
    assert!(state.can_stop());
    assert!(!state.can_start());
}

#[test]
fn unready_status_is_degraded_without_claiming_offline() {
    let state = classify_status(StatusResult {
        ready: false,
        healthy: false,
        daemon_version: "0.1.0".to_owned(),
        protocol_version: 6,
        queue_depth: 0,
    });

    assert!(matches!(state, HealthState::Degraded { .. }));
    assert!(!state.can_start());
    assert!(!state.can_stop());
}

#[test]
fn project_catalog_keeps_current_first_and_deduplicates_canonical_roots() {
    let current = project("pam", "/projects/pam", Some("project-pam"));
    let projects = merge_test_projects(
        current,
        vec![
            project("other", "/projects/other", None),
            project("PAM renamed", "/projects/pam", None),
            project("other duplicate", "/projects/other", None),
        ],
    );

    assert_eq!(projects.len(), 2);
    assert_eq!(projects[0].root, PathBuf::from("/projects/pam"));
    assert_eq!(projects[0].id, Some(ProjectId::new("project-pam")));
    assert_eq!(projects[1].root, PathBuf::from("/projects/other"));
}

#[test]
fn timeout_with_no_daemon_process_classifies_as_offline() {
    let state = super::control_center::health_from_timeout(
        Some(pam_platform::DaemonRuntimeState::NotRunning),
        &pam_daemon::ExchangeError::DeadlineExceeded,
    );
    assert_eq!(state, HealthState::Offline);
}

#[test]
fn timeout_with_a_live_daemon_reports_an_unresponsive_daemon() {
    let state = super::control_center::health_from_timeout(
        Some(pam_platform::DaemonRuntimeState::Running { pid: Some(4242) }),
        &pam_daemon::ExchangeError::DeadlineExceeded,
    );
    match state {
        HealthState::Degraded { detail, recovery } => {
            assert!(detail.contains("pid 4242"));
            assert!(detail.contains("did not respond in time"));
            assert!(recovery.is_some());
        }
        other => panic!("expected degraded, got {other:?}"),
    }
}

#[test]
fn timeout_with_an_unreadable_probe_keeps_the_original_error() {
    let state = super::control_center::health_from_timeout(
        None,
        &pam_daemon::ExchangeError::DeadlineExceeded,
    );
    match state {
        HealthState::Degraded { detail, .. } => {
            assert_eq!(detail, "PAM daemon request timed out.");
        }
        other => panic!("expected degraded, got {other:?}"),
    }
}
