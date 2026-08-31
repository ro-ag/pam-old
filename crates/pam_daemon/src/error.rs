use std::{error::Error, fmt};

use pam_model::RuntimeError;
use pam_platform::{IdentityError, TransportError};
use pam_protocol::CodecError;
use pam_store::StoreError;

use crate::reset::ResetError;

#[derive(Debug)]
pub enum DaemonError {
    AlreadyRunning,
    LaunchNotGranted,
    Handler(tokio::task::JoinError),
    Identity(IdentityError),
    StaleState(String),
    Io(std::io::Error),
    Model(RuntimeError),
    Protocol(CodecError),
    /// The reset root could not be resolved, so the daemon refuses to start
    /// rather than serve reset requests with no idea what they would delete.
    Reset(ResetError),
    Store(StoreError),
    Transport(TransportError),
}

impl DaemonError {
    #[must_use]
    pub const fn recovery_action(&self) -> Option<&'static str> {
        match self {
            Self::AlreadyRunning => Some("pam status"),
            Self::LaunchNotGranted | Self::StaleState(_) => Some("pam gui"),
            Self::Transport(error) => error.recovery_action(),
            Self::Handler(_)
            | Self::Identity(_)
            | Self::Io(_)
            | Self::Model(_)
            | Self::Protocol(_)
            | Self::Reset(_)
            | Self::Store(_) => None,
        }
    }
}

impl fmt::Display for DaemonError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyRunning => formatter
                .write_str("PAM daemon ownership is already claimed. Check it with `pam status`."),
            Self::LaunchNotGranted => formatter.write_str(
                "The PAM daemon starts only from the control center. Open it with `pam gui`.",
            ),
            Self::Handler(_) => formatter.write_str("PAM daemon request handling failed."),
            Self::Identity(error) => error.fmt(formatter),
            Self::StaleState(_) => formatter.write_str(
                "PAM daemon endpoint is stale. Restart PAM from the control center (`pam gui`).",
            ),
            Self::Io(_) => formatter.write_str("PAM could not prepare its local runtime state."),
            Self::Model(_) => {
                formatter.write_str("PAM could not start the embedded model runtime.")
            }
            Self::Protocol(_) => formatter.write_str("PAM could not process a protocol message."),
            Self::Reset(error) => error.fmt(formatter),
            Self::Store(_) => formatter.write_str("PAM durable state is unavailable."),
            Self::Transport(error) => error.fmt(formatter),
        }
    }
}

impl Error for DaemonError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Handler(error) => Some(error),
            Self::Identity(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::Model(error) => Some(error),
            Self::Protocol(error) => Some(error),
            Self::Reset(error) => Some(error),
            Self::Store(error) => Some(error),
            Self::Transport(error) => Some(error),
            Self::AlreadyRunning | Self::LaunchNotGranted | Self::StaleState(_) => None,
        }
    }
}

impl From<TransportError> for DaemonError {
    fn from(error: TransportError) -> Self {
        Self::Transport(error)
    }
}

impl From<CodecError> for DaemonError {
    fn from(error: CodecError) -> Self {
        Self::Protocol(error)
    }
}

impl From<StoreError> for DaemonError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<IdentityError> for DaemonError {
    fn from(error: IdentityError) -> Self {
        Self::Identity(error)
    }
}

impl From<std::io::Error> for DaemonError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<RuntimeError> for DaemonError {
    fn from(error: RuntimeError) -> Self {
        Self::Model(error)
    }
}
