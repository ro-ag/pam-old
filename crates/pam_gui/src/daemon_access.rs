//! Daemon-scope capability grants the GUI writes for its own caller.
//!
//! The daemon-scoped capabilities this GUI calls are deliberately not
//! baseline, so policy denies them until the owner grants them. This module
//! is the only surface that writes those grants, and it writes one only when
//! the owner asks: nothing here runs on first run or as a side effect of any
//! other command.

use std::{
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use pam_core::{CallerId, GrantId, ProjectId};
use pam_policy::{ApprovalRequirement, CapabilityName, Effect, Grant, ResourceScope};
use pam_store::{PutGrant, Store, StoreError};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    desktop::{DesktopErrorDto, DesktopResult},
    store_writes::{StoreWriteFailure, revoke_capability},
};

/// The daemon-scoped capabilities the GUI itself calls, with the surface each
/// one serves. Every entry must stay outside
/// [`pam_policy::BASELINE_CAPABILITIES`]: revoking a row drops every grant
/// for that capability, which only reads as "not granted" while default-deny
/// still applies.
pub(crate) const GUI_DAEMON_CAPABILITIES: [(&str, &str, &str); 15] = [
    (
        "model.infer",
        "Model inference",
        "Chat and the Models view model check ask the loaded model to generate.",
    ),
    (
        "model.register",
        "Model registration",
        "Models registers an imported or downloaded GGUF in the daemon's registry.",
    ),
    (
        "model.load",
        "Model loading",
        "Models brings a registered model into the running daemon, replacing whatever it was serving.",
    ),
    (
        "model.unload",
        "Model unloading",
        "Models drops the loaded model and frees its memory; Pam keeps serving.",
    ),
    (
        "model.unregister",
        "Model removal",
        "Models removes a registered model from the daemon's registry; the weights stay on disk.",
    ),
    (
        "model.verify",
        "Model verification",
        "Models re-reads registered weights and reports what no longer matches the registry.",
    ),
    (
        "model.sweep",
        "Model directory sweep",
        "Models reconciles the registry against the models directory and reports what it costs.",
    ),
    (
        "model.delete-weights",
        "Model weights deletion",
        "Models deletes a GGUF Pam downloaded into its own models directory and unregisters it.",
    ),
    (
        "network.diagnostics",
        "Access boundary read",
        "Access reads the daemon's observed TLS roots, proxy environment, and PAC state.",
    ),
    (
        "connector.configure",
        "Connector configuration",
        "Access saves a connector's enablement, base URL, and credential.",
    ),
    (
        "connector.test",
        "Connector self-test",
        "Access runs a connector's self-test against its configured host.",
    ),
    (
        "reset.access",
        "Reset grants and approvals",
        "The Settings danger zone revokes every capability grant and approval.",
    ),
    (
        "reset.identity",
        "Reset caller identity",
        "The Settings danger zone revokes every caller and purges its keychain entry.",
    ),
    (
        "reset.history",
        "Clear history",
        "The Settings danger zone clears the audit ledger, evidence, and flow-run history.",
    ),
    (
        "reset.registry",
        "Reset the model registry",
        "The Settings danger zone unregisters every model, leaving weights on disk.",
    ),
];

/// The GUI caller's daemon-scope grant state, one row per capability.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DaemonAccessDto {
    pub capabilities: Vec<DaemonCapabilityDto>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DaemonCapabilityDto {
    pub capability: String,
    pub name: String,
    pub summary: String,
    pub granted: bool,
}

/// Reads the GUI caller's daemon-scope grant state.
///
/// # Errors
///
/// Returns a bounded error when durable state is unavailable or corrupt.
pub(crate) async fn read_daemon_access(
    state_path: PathBuf,
    caller: CallerId,
) -> DesktopResult<DaemonAccessDto> {
    let now = now_ms()?;
    let store = open_store(&state_path)?;
    let grants = store
        .active_grants(caller, ProjectId::daemon_scope(), now)
        .await;
    let shutdown = store.shutdown().await;
    let grants = grants.map_err(store_error)?;
    shutdown.map_err(store_error)?;
    Ok(access_dto(&grants))
}

/// Grants or revokes one daemon-scope capability for the GUI caller.
///
/// Granting writes one unconditional allow over any resource; revoking drops
/// every active grant for that capability, returning it to default-deny.
///
/// # Errors
///
/// Returns a bounded error for a capability the GUI does not use, or when
/// durable state is unavailable, corrupt, or has no registered GUI caller.
pub(crate) async fn update_daemon_access(
    state_path: PathBuf,
    caller: CallerId,
    capability: String,
    granted: bool,
    daemon_owns_store: bool,
) -> DesktopResult<DaemonAccessDto> {
    let capability = known_capability(&capability)?;
    let now = now_ms()?;
    let store = open_store(&state_path)?;
    let grants = apply(
        &store,
        &caller,
        &capability,
        granted,
        now,
        daemon_owns_store,
    )
    .await;
    let shutdown = store.shutdown().await;
    let grants = grants?;
    shutdown.map_err(store_error)?;
    Ok(access_dto(&grants))
}

async fn apply(
    store: &Store,
    caller: &CallerId,
    capability: &CapabilityName,
    granted: bool,
    now_ms: u64,
    daemon_owns_store: bool,
) -> DesktopResult<Vec<Grant>> {
    let project = ProjectId::daemon_scope();
    let active = store
        .active_grants(caller.clone(), project.clone(), now_ms)
        .await
        .map_err(write_error)?;
    if granted && is_granted(&active, capability) {
        return Ok(active);
    }
    if active.iter().any(|grant| grant.capability == *capability) {
        // Revocation is routed to the daemon while it owns the store.
        revoke_capability(store, caller, capability, now_ms, daemon_owns_store)
            .await
            .map_err(revoke_error)?;
    }
    if granted {
        // Granting stays a direct write: a caller cannot be handed the
        // authority to grant itself authority, so this bootstrap has no
        // protocol equivalent to route to.
        store
            .put_grant(PutGrant {
                grant: Grant {
                    id: GrantId::new(Uuid::new_v4().to_string()),
                    caller: caller.clone(),
                    project: project.clone(),
                    capability: capability.clone(),
                    resource: ResourceScope::Any,
                    effect: Effect::Allow,
                    approval: ApprovalRequirement::None,
                    expires_at_ms: None,
                    revoked_at_ms: None,
                },
                created_at_ms: now_ms,
            })
            .await
            .map_err(write_error)?;
    }
    store
        .active_grants(caller.clone(), project, now_ms)
        .await
        .map_err(write_error)
}

/// Mirrors deny-overrides policy for a request over any resource: an explicit
/// deny wins, and only an unconditional allow over any resource counts.
fn is_granted(grants: &[Grant], capability: &CapabilityName) -> bool {
    let mut allowed = false;
    for grant in grants
        .iter()
        .filter(|grant| grant.capability == *capability)
    {
        match grant.effect {
            Effect::Deny => return false,
            Effect::Allow => {
                allowed |= grant.approval == ApprovalRequirement::None
                    && grant.resource == ResourceScope::Any;
            }
        }
    }
    allowed
}

fn access_dto(grants: &[Grant]) -> DaemonAccessDto {
    DaemonAccessDto {
        capabilities: GUI_DAEMON_CAPABILITIES
            .iter()
            .map(|(capability, name, summary)| DaemonCapabilityDto {
                capability: (*capability).to_owned(),
                name: (*name).to_owned(),
                summary: (*summary).to_owned(),
                granted: CapabilityName::parse(*capability)
                    .is_ok_and(|capability| is_granted(grants, &capability)),
            })
            .collect(),
    }
}

fn known_capability(capability: &str) -> DesktopResult<CapabilityName> {
    if !GUI_DAEMON_CAPABILITIES
        .iter()
        .any(|(known, _, _)| *known == capability)
    {
        return Err(DesktopErrorDto::invalid_input(
            "This is not a daemon-scoped capability the Pam window uses.",
        ));
    }
    CapabilityName::parse(capability)
        .map_err(|error| DesktopErrorDto::invalid_input(error.to_string()))
}

fn open_store(state_path: &Path) -> DesktopResult<Store> {
    Store::open(state_path).map_err(store_error)
}

fn now_ms() -> DesktopResult<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
        .ok_or_else(|| {
            DesktopErrorDto::unavailable(
                "The host clock is before the Unix epoch.",
                Some("Correct the system clock, then retry.".to_owned()),
            )
        })
}

fn store_error(_error: StoreError) -> DesktopErrorDto {
    DesktopErrorDto::unavailable(
        "Pam could not read its durable capability grants.",
        Some("Verify the local Pam state directory and retry.".to_owned()),
    )
}

/// A refused revocation keeps the daemon's own words: the owner needs to see
/// that a running daemon declined, not a generic store message.
fn revoke_error(failure: StoreWriteFailure) -> DesktopErrorDto {
    DesktopErrorDto::unavailable(failure.detail, failure.recovery)
}

fn write_error(_error: StoreError) -> DesktopErrorDto {
    DesktopErrorDto::unavailable(
        "Pam could not record this capability grant.",
        Some("Register this Pam window as a caller from Access, then retry.".to_owned()),
    )
}
