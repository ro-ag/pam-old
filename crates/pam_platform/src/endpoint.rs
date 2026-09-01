use std::{env, path::PathBuf};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalEndpoint {
    address: String,
    runtime_dir: PathBuf,
    socket_path: Option<PathBuf>,
    ownership_path: PathBuf,
}

impl LocalEndpoint {
    /// Returns the default per-user local IPC endpoint.
    ///
    /// # Panics
    ///
    /// Panics when the operating system exposes neither a session runtime
    /// directory nor the per-user local-data directory required by every
    /// supported Pam platform. `PAM_RUNTIME_DIR` can provide an explicit
    /// absolute override for constrained environments.
    #[must_use]
    pub fn default_for_user() -> Self {
        Self::ipc(runtime_dir())
    }

    #[must_use]
    pub fn ipc(runtime_dir: PathBuf) -> Self {
        let socket_path = runtime_dir.join("daemon.sock");
        Self {
            address: format!("ipc://{}", socket_path.display()),
            socket_path: Some(socket_path),
            ownership_path: runtime_dir.join("daemon.lock"),
            runtime_dir,
        }
    }

    #[must_use]
    pub fn loopback(address: impl Into<String>, runtime_dir: PathBuf) -> Self {
        Self {
            address: address.into(),
            socket_path: None,
            ownership_path: runtime_dir.join("daemon.lock"),
            runtime_dir,
        }
    }

    #[must_use]
    pub fn address(&self) -> &str {
        &self.address
    }

    #[must_use]
    pub fn socket_path(&self) -> Option<&std::path::Path> {
        self.socket_path.as_deref()
    }

    #[must_use]
    pub fn ownership_path(&self) -> &std::path::Path {
        &self.ownership_path
    }

    #[must_use]
    pub fn runtime_dir(&self) -> &std::path::Path {
        &self.runtime_dir
    }
}

fn runtime_dir() -> PathBuf {
    if let Some(configured) = env::var_os("PAM_RUNTIME_DIR") {
        return PathBuf::from(configured);
    }

    if !cfg!(windows)
        && let Some(xdg_runtime_dir) = env::var_os("XDG_RUNTIME_DIR")
    {
        return PathBuf::from(xdg_runtime_dir).join("pam");
    }

    private_runtime_dir()
        .expect("supported Pam platforms must provide a private per-user local-data directory")
}

pub(super) fn private_runtime_dir() -> Option<PathBuf> {
    crate::data_dir::project_dirs()
        .map(|project_dirs| project_dirs.data_local_dir().join("runtime"))
}

/// Observed liveness of the local daemon, derived from its ownership lock.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DaemonRuntimeState {
    /// The ownership lock is held: a daemon process is alive.
    Running { pid: Option<u32> },
    /// No process holds the ownership lock; stale socket or lock residue may
    /// remain, which a `--recover` launch clears.
    NotRunning,
}

/// Probes daemon liveness through the ownership lock without disturbing a
/// running daemon. Returns `None` when the artifacts cannot be inspected.
///
/// The probe briefly takes the free lock to prove no daemon holds it, so a
/// caller that is about to spawn a daemon must not probe concurrently: the
/// spawned daemon could observe the probe's transient hold and abort as
/// already running. The GUI serializes daemon commands, which satisfies this.
#[must_use]
pub fn probe_daemon_runtime(endpoint: &LocalEndpoint) -> Option<DaemonRuntimeState> {
    let path = endpoint.ownership_path();
    if !path.exists() {
        return Some(DaemonRuntimeState::NotRunning);
    }
    let file = std::fs::File::open(path).ok()?;
    match file.try_lock() {
        Ok(()) => Some(DaemonRuntimeState::NotRunning),
        Err(std::fs::TryLockError::WouldBlock) => {
            let pid = std::fs::read_to_string(path)
                .ok()
                .and_then(|content| content.trim().parse::<u32>().ok());
            Some(DaemonRuntimeState::Running { pid })
        }
        Err(std::fs::TryLockError::Error(_)) => None,
    }
}

/// Single-use file carrying the nonce that authorizes one daemon launch.
pub const LAUNCH_GRANT_FILE: &str = "launch-grant";

/// Environment variable through which the launcher presents the nonce.
pub const LAUNCH_GRANT_ENV: &str = "PAM_LAUNCH_GRANT";

/// Issues a single-use daemon launch grant under the runtime directory and
/// returns the nonce the launcher must present via [`LAUNCH_GRANT_ENV`].
///
/// # Errors
///
/// Returns the underlying I/O error when the runtime directory or grant file
/// cannot be created.
pub fn issue_launch_grant(runtime_dir: &std::path::Path) -> std::io::Result<String> {
    std::fs::create_dir_all(runtime_dir)?;
    let nonce = uuid::Uuid::new_v4().to_string();
    let path = runtime_dir.join(LAUNCH_GRANT_FILE);
    std::fs::write(&path, &nonce)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(nonce)
}

/// Consumes the pending launch grant when the presented nonce matches.
///
/// A match deletes the grant file so every grant is single-use; a mismatch
/// leaves the pending grant untouched for the legitimate launcher.
#[must_use]
pub fn consume_launch_grant(runtime_dir: &std::path::Path, presented: Option<&str>) -> bool {
    let Some(presented) = presented else {
        return false;
    };
    let path = runtime_dir.join(LAUNCH_GRANT_FILE);
    let Ok(expected) = std::fs::read_to_string(&path) else {
        return false;
    };
    if expected.trim() != presented {
        return false;
    }
    std::fs::remove_file(&path).is_ok()
}
