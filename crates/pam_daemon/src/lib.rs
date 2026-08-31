#![forbid(unsafe_code)]

mod connectors;
mod error;
mod flow;
mod lifecycle;
mod logging;
#[cfg(target_os = "macos")]
mod macos_admission;
mod model_service;
mod ptrack;
mod reset;

#[cfg(test)]
mod connectors_test;
#[cfg(test)]
mod flow_test;
#[cfg(test)]
mod lifecycle_test;
#[cfg(test)]
mod logging_test;
#[cfg(all(test, target_os = "macos"))]
mod macos_admission_test;
#[cfg(test)]
mod model_service_test;
#[cfg(test)]
mod ptrack_test;
#[cfg(test)]
mod reset_test;

pub use error::DaemonError;
pub use lifecycle::{BriefProvider, ConnectorSecretOverride, DaemonConfig, run, serve_until};
pub use ptrack::{RegisteredProject, registered_projects};
pub use reset::{
    CONFIRMATION_RECOVERY, CredentialStore, DAEMON_RUNNING_RECOVERY, FactoryReceipt,
    FactoryResetOptions, ResetContext, ResetError, ResetPaths, daemon_owns_store,
    preview_factory_reset, run_factory_reset, run_tier,
};
