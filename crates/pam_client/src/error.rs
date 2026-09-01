use std::{error::Error, fmt};

use pam_platform::TransportError;
use pam_protocol::CodecError;

#[derive(Debug)]
pub enum ExchangeError {
    Correlation(String),
    DeadlineExceeded,
    EventLimitExceeded,
    Protocol(CodecError),
    Transport(TransportError),
}

impl ExchangeError {
    #[must_use]
    pub fn is_unavailable(&self) -> bool {
        matches!(
            self,
            Self::Transport(error)
                if error.kind() == pam_platform::TransportErrorKind::Unavailable
        )
    }

    #[must_use]
    pub const fn recovery_action(&self) -> Option<&'static str> {
        match self {
            Self::Transport(error) => error.recovery_action(),
            Self::Correlation(_)
            | Self::DeadlineExceeded
            | Self::EventLimitExceeded
            | Self::Protocol(_) => None,
        }
    }
}

impl fmt::Display for ExchangeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Correlation(_) => {
                formatter.write_str("Pam daemon returned an uncorrelated response.")
            }
            Self::DeadlineExceeded => formatter.write_str("Pam daemon request timed out."),
            Self::EventLimitExceeded => {
                formatter.write_str("Pam daemon response exceeded the event limit.")
            }
            Self::Protocol(_) => formatter.write_str("Pam daemon returned an invalid response."),
            Self::Transport(error) => error.fmt(formatter),
        }
    }
}

impl Error for ExchangeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Protocol(error) => Some(error),
            Self::Transport(error) => Some(error),
            Self::Correlation(_) | Self::DeadlineExceeded | Self::EventLimitExceeded => None,
        }
    }
}

impl From<CodecError> for ExchangeError {
    fn from(error: CodecError) -> Self {
        Self::Protocol(error)
    }
}

impl From<TransportError> for ExchangeError {
    fn from(error: TransportError) -> Self {
        Self::Transport(error)
    }
}

pub type StatusError = ExchangeError;
