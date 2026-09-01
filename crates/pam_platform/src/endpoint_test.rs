use std::io::Write as _;
use std::path::PathBuf;

use super::{
    LocalEndpoint,
    endpoint::{
        DaemonRuntimeState, LAUNCH_GRANT_FILE, consume_launch_grant, issue_launch_grant,
        private_runtime_dir, probe_daemon_runtime,
    },
};

#[test]
fn ipc_endpoint_keeps_transport_and_ownership_paths_together() {
    let endpoint = LocalEndpoint::ipc(PathBuf::from("/tmp/pam-endpoint-test"));

    assert_eq!(
        endpoint.address(),
        "ipc:///tmp/pam-endpoint-test/daemon.sock"
    );
    assert_eq!(
        endpoint.socket_path(),
        Some(std::path::Path::new("/tmp/pam-endpoint-test/daemon.sock"))
    );
    assert_eq!(
        endpoint.ownership_path(),
        std::path::Path::new("/tmp/pam-endpoint-test/daemon.lock")
    );
}

#[test]
fn default_endpoint_uses_local_ipc() {
    let endpoint = LocalEndpoint::default_for_user();
    let socket_path = endpoint
        .socket_path()
        .expect("the default endpoint must use a local IPC socket");

    assert_eq!(socket_path, endpoint.runtime_dir().join("daemon.sock"));
    assert_eq!(
        endpoint.address(),
        format!("ipc://{}", socket_path.display())
    );
}

#[test]
fn fallback_runtime_is_rooted_in_private_per_user_data() {
    let project_dirs = directories::ProjectDirs::from("dev", "pam", "pam")
        .expect("the test host must expose a per-user data directory");

    assert_eq!(
        private_runtime_dir(),
        Some(project_dirs.data_local_dir().join("runtime"))
    );
}

#[test]
fn launch_grant_round_trip_consumes_the_grant_file() {
    let dir = std::env::temp_dir().join(format!("pam-grant-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();

    let nonce = issue_launch_grant(&dir).unwrap();
    assert!(dir.join(LAUNCH_GRANT_FILE).exists());
    assert!(consume_launch_grant(&dir, Some(&nonce)));
    assert!(
        !dir.join(LAUNCH_GRANT_FILE).exists(),
        "a grant must be single-use"
    );

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn launch_grant_rejects_missing_mismatched_and_replayed_presentations() {
    let dir = std::env::temp_dir().join(format!("pam-grant-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();

    assert!(!consume_launch_grant(&dir, None));
    assert!(!consume_launch_grant(&dir, Some("ungranted")));

    let nonce = issue_launch_grant(&dir).unwrap();
    assert!(!consume_launch_grant(&dir, Some("wrong-nonce")));
    assert!(
        dir.join(LAUNCH_GRANT_FILE).exists(),
        "a mismatch must not consume the real grant"
    );
    assert!(consume_launch_grant(&dir, Some(&nonce)));
    assert!(!consume_launch_grant(&dir, Some(&nonce)), "no replay");

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn probe_tracks_the_ownership_lock_lifecycle() {
    let dir = std::env::temp_dir().join(format!(
        "pam-probe-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let endpoint = LocalEndpoint::ipc(dir.clone());

    // No artifacts at all: nothing runs.
    assert_eq!(
        probe_daemon_runtime(&endpoint),
        Some(DaemonRuntimeState::NotRunning)
    );

    // A held exclusive lock with a pid is a live daemon.
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(endpoint.ownership_path())
        .unwrap();
    writeln!(&lock, "4242").unwrap();
    lock.try_lock().unwrap();
    assert_eq!(
        probe_daemon_runtime(&endpoint),
        Some(DaemonRuntimeState::Running { pid: Some(4242) })
    );

    // Releasing the lock leaves only residue: not running again. The kernel
    // may briefly report the flock as held after close, so poll bounded.
    drop(lock);
    let mut released = probe_daemon_runtime(&endpoint);
    for _ in 0..50 {
        if released == Some(DaemonRuntimeState::NotRunning) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
        released = probe_daemon_runtime(&endpoint);
    }
    assert_eq!(released, Some(DaemonRuntimeState::NotRunning));

    std::fs::remove_dir_all(dir).unwrap();
}
