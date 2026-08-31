//! GUI writes to durable state, routed to whoever owns that state.
//!
//! The daemon owns the durable store while it runs, so every GUI write goes
//! through the protocol whenever a daemon holds the endpoint's ownership lock.
//! A running daemon that refuses the request is a real refusal: it is reported
//! as-is, never raced with a direct write.
//!
//! When no daemon holds the lock, nothing owns the store and the GUI writes
//! directly. That fallback is required, not a convenience: importing a GGUF
//! and then starting the daemon with it is the first-run bootstrap, so model
//! registration has to keep working with the daemon stopped. Grant revocation
//! shares the same rule, which keeps the Access controls usable before the
//! daemon has ever started.

use pam_core::{CallerCredential, CallerId, ProjectId};
use pam_model::{ModelSource, RegisteredModel};
use pam_platform::{CallerKind, DaemonRuntimeState, LocalEndpoint, caller_id, user_data_dir};
use pam_policy::CapabilityName;
use pam_protocol::ModelRegistration;
use pam_store::Store;

use crate::{
    control_center::load_credential,
    observatory::{ObservatoryState, run_grant_revoke, run_model_register},
};

const STORE_RECOVERY: &str = "Verify the local PAM data store, then retry.";
const REGISTRATION_RECOVERY: &str = "Retry the registration once the PAM daemon is reachable.";
const REVOCATION_RECOVERY: &str = "Retry the revocation once the PAM daemon is reachable.";
const CALLER_RECOVERY: &str = "Use Register GUI caller in PAM, then retry.";

/// A bounded, user-facing durable-write failure.
#[derive(Clone, Debug)]
pub(crate) struct StoreWriteFailure {
    pub(crate) detail: String,
    pub(crate) recovery: Option<String>,
}

impl StoreWriteFailure {
    fn new(detail: impl Into<String>, recovery: &str) -> Self {
        Self {
            detail: detail.into(),
            recovery: Some(recovery.to_owned()),
        }
    }
}

/// Whether a live daemon holds the endpoint's ownership lock, and therefore
/// owns the durable store.
///
/// An unreadable lock is treated as owned: an unknown owner is never a licence
/// to become a second writer, and the routed request then fails honestly
/// instead of racing whatever holds it.
pub(crate) fn daemon_owns_store() -> bool {
    owns_store(pam_platform::probe_daemon_runtime(
        &LocalEndpoint::default_for_user(),
    ))
}

/// The ownership rule itself, over an observed lock state.
pub(crate) const fn owns_store(runtime: Option<DaemonRuntimeState>) -> bool {
    !matches!(runtime, Some(DaemonRuntimeState::NotRunning))
}

/// Registers one verified model, through the daemon when it owns the store.
///
/// The daemon's acknowledgement carries the stored identity only: registration
/// is idempotent over an identical artifact and conflicts on any other, so an
/// accepted registration is exactly the record submitted here.
pub(crate) async fn register_model(
    model: RegisteredModel,
) -> Result<RegisteredModel, StoreWriteFailure> {
    if !daemon_owns_store() {
        return put_model_directly(model).await;
    }
    let (caller, credential) = gui_caller().await?;
    let registration = registration_of(&model);
    let observed = run_model_register(caller, credential, registration)
        .await
        .map_err(|error| StoreWriteFailure::new(error.to_string(), REGISTRATION_RECOVERY))?;
    let acknowledged = available(observed, REGISTRATION_RECOVERY)?;
    if acknowledged.model == model.key.id() {
        Ok(model)
    } else {
        Err(StoreWriteFailure::new(
            "PAM registered a different model than the one submitted.",
            REGISTRATION_RECOVERY,
        ))
    }
}

/// Revokes every daemon-scope grant the GUI caller holds for one capability,
/// through the daemon when it owns the store.
pub(crate) async fn revoke_capability(
    store: &Store,
    caller: &CallerId,
    capability: &CapabilityName,
    now_ms: u64,
    daemon_owns_store: bool,
) -> Result<(), StoreWriteFailure> {
    if daemon_owns_store {
        let credential = load_credential(caller.clone())
            .await
            .map_err(|detail| StoreWriteFailure::new(detail, CALLER_RECOVERY))?;
        let observed = run_grant_revoke(caller.clone(), credential, capability.as_str().to_owned())
            .await
            .map_err(|error| StoreWriteFailure::new(error.to_string(), REVOCATION_RECOVERY))?;
        available(observed, REVOCATION_RECOVERY)?;
        return Ok(());
    }
    let active = store
        .active_grants(caller.clone(), ProjectId::daemon_scope(), now_ms)
        .await
        .map_err(|error| StoreWriteFailure::new(error.to_string(), STORE_RECOVERY))?;
    for grant in active
        .iter()
        .filter(|grant| grant.capability == *capability)
    {
        store
            .revoke_grant(grant.id.clone(), now_ms)
            .await
            .map_err(|error| StoreWriteFailure::new(error.to_string(), STORE_RECOVERY))?;
    }
    Ok(())
}

/// The wire form of one registered model, field for field.
pub(crate) fn registration_of(model: &RegisteredModel) -> ModelRegistration {
    ModelRegistration {
        model: model.key.id(),
        path: model.path.to_string_lossy().into_owned(),
        digest: model.digest.as_str().to_owned(),
        size_bytes: model.size_bytes,
        gguf_version: model.gguf.version,
        gguf_tensor_count: model.gguf.tensor_count,
        gguf_metadata_kv_count: model.gguf.metadata_kv_count,
        license_id: model.license.identifier().to_owned(),
        license_url: model.license.notice_url().to_owned(),
        license_digest: model.license.notice_digest().as_str().to_owned(),
        source_url: match &model.source {
            ModelSource::Local => None,
            ModelSource::Https { canonical_url } => Some(canonical_url.clone()),
        },
        registered_at_ms: model.registered_at_ms,
    }
}

fn available<T>(
    observed: ObservatoryState<T>,
    default_recovery: &str,
) -> Result<T, StoreWriteFailure> {
    match observed {
        ObservatoryState::Available(result) => Ok(result),
        ObservatoryState::Blocked {
            detail, recovery, ..
        }
        | ObservatoryState::Unavailable {
            detail, recovery, ..
        } => Err(StoreWriteFailure {
            detail,
            recovery: recovery.or_else(|| Some(default_recovery.to_owned())),
        }),
    }
}

async fn gui_caller() -> Result<(CallerId, CallerCredential), StoreWriteFailure> {
    let caller = caller_id(CallerKind::Gui)
        .map_err(|error| StoreWriteFailure::new(error.to_string(), CALLER_RECOVERY))?;
    let credential = load_credential(caller.clone())
        .await
        .map_err(|detail| StoreWriteFailure::new(detail, CALLER_RECOVERY))?;
    Ok((caller, credential))
}

async fn put_model_directly(model: RegisteredModel) -> Result<RegisteredModel, StoreWriteFailure> {
    let state_path = user_data_dir()
        .map_err(|_| {
            StoreWriteFailure::new(
                "PAM could not resolve its local data store.",
                "Verify the operating system user data directory, then retry.",
            )
        })?
        .join("state.sqlite3");
    let store = Store::open(state_path)
        .map_err(|error| StoreWriteFailure::new(error.to_string(), STORE_RECOVERY))?;
    let result = store.put_model(model).await;
    let shutdown = store.shutdown().await;
    let registered =
        result.map_err(|error| StoreWriteFailure::new(error.to_string(), STORE_RECOVERY))?;
    shutdown.map_err(|error| StoreWriteFailure::new(error.to_string(), STORE_RECOVERY))?;
    Ok(registered)
}
