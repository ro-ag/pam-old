//! Tiered reset of PAM's durable state.
//!
//! PAM's state lives in five places with very different blast radii, so reset
//! is four scoped operations and never one nuke button: `access` drops grants
//! and approvals, `identity` forces every caller to re-pair, `history` clears
//! the audit ledger, the evidence store, and flow-run history, and `registry`
//! unregisters models while leaving every byte of their weights alone. A
//! factory reset performs all four, resets settings to defaults, and removes
//! the authored flow library as well.
//!
//! Two rules hold everywhere in this module, because a reset that deletes the
//! wrong directory is unrecoverable:
//!
//! 1. Every path is derived from [`pam_platform::user_data_dir`] (or, for the
//!    daemon's own test override, from the state database it was given) and
//!    joined from single validated path components. Nothing is assembled from
//!    a caller-supplied string.
//! 2. Nothing is removed before [`ensure_within`] proves it is inside the
//!    resolved root, and no removal ever follows a symlink: a link is
//!    unlinked, never descended.

use std::{
    error::Error,
    fmt::{self, Write as _},
    fs, io,
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use pam_core::{CallerId, ProjectId};
use pam_platform::{
    DaemonRuntimeState, IdentityError, LocalEndpoint, NativeSecretBackend, SecretBackend,
    SecretBackendError, SecretLocator, probe_daemon_runtime, user_data_dir,
};
use pam_policy::redact_audit_detail;
use pam_protocol::{ResetItem, ResetResult, ResetTier};
use pam_store::{AppendAuditEvent, ResetTally, Store, StoreError};

/// How long a reset's own audit event is retained, matching the daemon's
/// ledger retention.
const AUDIT_RETENTION: Duration = Duration::from_hours(30 * 24);
/// One page of evidence handles per purge round trip; the store rejects more.
const EVIDENCE_PURGE_PAGE: u32 = 1_000;
/// Hard ceiling on purge rounds so a pathological store can never spin here.
const EVIDENCE_PURGE_ROUNDS: u32 = 8_192;
/// Newest audit events scanned when confirming the reset recorded itself.
const AUDIT_READBACK_LIMIT: u32 = 16;

const STATE_FILE: &str = "state.sqlite3";
const SETTINGS_FILE: &str = "settings.json";
const CALLERS_DIRECTORY: &str = "callers";
const EVIDENCE_DIRECTORY: &str = "evidence";
const LOGS_DIRECTORY: &str = "logs";
const RUNTIME_DIRECTORY: &str = "runtime";
/// The flow library's relative location under the data root, the same layout
/// [`pam_platform::flow_library_root`] opens beneath.
const FLOW_LIBRARY: [&str; 2] = [".pam", "flows"];
const RECEIPT_PREFIX: &str = "pam-reset-receipt-";

/// Recovery text for a reset refused because a daemon still owns the store.
pub const DAEMON_RUNNING_RECOVERY: &str = "Stop PAM first -- quit the running `pam daemon`, or press Stop in the PAM control center -- then run the reset again.";
/// Recovery text for a reset that would change state without confirmation.
pub const CONFIRMATION_RECOVERY: &str =
    "Re-run with --dry-run to see exactly what would go, then with --yes to perform it.";

/// Whether a live daemon holds this endpoint's ownership lock, and therefore
/// owns the durable store.
///
/// An unreadable lock counts as owned: an unknown owner is never a licence to
/// start deleting the state it may be writing.
#[must_use]
pub fn daemon_owns_store(endpoint: &LocalEndpoint) -> bool {
    !matches!(
        probe_daemon_runtime(endpoint),
        Some(DaemonRuntimeState::NotRunning)
    )
}

/// Every way a reset can refuse or fail.
#[derive(Debug)]
pub enum ResetError {
    /// The operating system did not expose a user data directory.
    DataDirectory(IdentityError),
    /// Durable state was unavailable or rejected the change.
    Store(StoreError),
    /// A path could not be inspected or removed.
    Filesystem { path: PathBuf, source: io::Error },
    /// A path resolved outside the data root and was not touched.
    OutsideRoot(PathBuf),
    /// The native credential store refused or was unavailable.
    Credentials(SecretBackendError),
    /// A daemon still owns the store, so a factory reset cannot proceed.
    DaemonRunning,
    /// A blocking reset step could not be joined.
    Interrupted,
}

impl ResetError {
    /// The one concrete next step that clears this refusal, when there is one.
    #[must_use]
    pub fn recovery(&self) -> Option<String> {
        match self {
            Self::DaemonRunning => Some(DAEMON_RUNNING_RECOVERY.to_owned()),
            Self::Filesystem { path, .. } => Some(format!(
                "Close any program holding {}, then run the reset again.",
                path.display()
            )),
            Self::OutsideRoot(_) => Some(
                "Reset refused a path outside PAM's data directory and changed nothing. Report this."
                    .to_owned(),
            ),
            Self::Credentials(_) => Some(
                "Unlock the login keychain and allow PAM to access its credentials, then run the reset again."
                    .to_owned(),
            ),
            Self::DataDirectory(_) | Self::Store(_) | Self::Interrupted => None,
        }
    }
}

impl fmt::Display for ResetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DataDirectory(error) => {
                write!(
                    formatter,
                    "PAM could not resolve its data directory: {error}"
                )
            }
            Self::Store(error) => write!(formatter, "durable state was unavailable: {error}"),
            Self::Filesystem { path, source } => {
                write!(formatter, "could not remove {}: {source}", path.display())
            }
            Self::OutsideRoot(path) => write!(
                formatter,
                "refused to touch {} because it is outside PAM's data directory",
                path.display()
            ),
            Self::Credentials(error) => {
                write!(formatter, "the credential store was unavailable: {error}")
            }
            Self::DaemonRunning => {
                formatter.write_str("a running daemon still owns PAM's durable state")
            }
            Self::Interrupted => formatter.write_str("the reset was interrupted before it ran"),
        }
    }
}

impl Error for ResetError {}

impl From<StoreError> for ResetError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<SecretBackendError> for ResetError {
    fn from(error: SecretBackendError) -> Self {
        Self::Credentials(error)
    }
}

/// The native credential store a reset purges caller material from.
///
/// Caller credentials and connector credentials share one credential store,
/// so this is the same backend the daemon's connector runtime uses and the
/// same seam its tests already inject.
#[derive(Clone)]
pub enum CredentialStore {
    /// The operating system's own credential store.
    Native,
    /// An injected backend, for isolated tests and for a daemon started with
    /// a credential-store override.
    Injected(Arc<dyn SecretBackend + Send + Sync>),
}

impl fmt::Debug for CredentialStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CredentialStore([REDACTED])")
    }
}

/// Every path a reset may read or remove, all rooted in one resolved
/// directory that no caller supplies.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResetPaths {
    root: PathBuf,
}

impl ResetPaths {
    /// Resolves the platform's user data directory as the reset root.
    ///
    /// # Errors
    ///
    /// Returns an error when the operating system exposes no user data
    /// directory for this process.
    pub fn discover() -> Result<Self, ResetError> {
        let root = user_data_dir().map_err(ResetError::DataDirectory)?;
        Ok(Self::rooted(root))
    }

    /// Resolves the root from the state database the daemon actually opened.
    ///
    /// The state file always sits directly in the data directory, so its
    /// parent is that directory -- including when a test points the daemon at
    /// a scratch directory instead of the platform one.
    ///
    /// # Errors
    ///
    /// Returns an error when the state path has no parent directory.
    pub fn for_state_path(state_path: &Path) -> Result<Self, ResetError> {
        let parent = state_path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .ok_or_else(|| ResetError::OutsideRoot(state_path.to_path_buf()))?;
        Ok(Self::rooted(parent.to_path_buf()))
    }

    /// Canonicalizes the root when it exists so containment checks compare
    /// real paths; an absent root is kept as given and simply has nothing to
    /// remove.
    fn rooted(root: PathBuf) -> Self {
        let root = fs::canonicalize(&root).unwrap_or(root);
        Self { root }
    }

    /// The resolved data root. Never removed itself; only its contents are.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The durable state database this root holds.
    #[must_use]
    pub fn state_path(&self) -> PathBuf {
        self.root.join(STATE_FILE)
    }

    fn child(&self, segment: &str) -> Result<PathBuf, ResetError> {
        let child = self.root.join(segment);
        ensure_single_component(segment, &child)?;
        Ok(child)
    }

    fn flow_library(&self) -> Result<PathBuf, ResetError> {
        let mut path = self.root.clone();
        for segment in FLOW_LIBRARY {
            ensure_single_component(segment, &path)?;
            path.push(segment);
        }
        Ok(path)
    }

    /// Where a factory-reset receipt is written: beside the data root rather
    /// than inside it, so the wipe cannot delete its own record.
    fn receipt_path(&self, now_ms: u64) -> Result<PathBuf, ResetError> {
        let parent = self
            .root
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .ok_or_else(|| ResetError::OutsideRoot(self.root.clone()))?;
        Ok(parent.join(format!("{RECEIPT_PREFIX}{now_ms}.txt")))
    }
}

fn ensure_single_component(segment: &str, context: &Path) -> Result<(), ResetError> {
    let mut components = Path::new(segment).components();
    let single =
        matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none();
    if single {
        Ok(())
    } else {
        Err(ResetError::OutsideRoot(context.to_path_buf()))
    }
}

/// Proves `target` is strictly inside `root` before anything is removed.
///
/// The root itself is never a valid target: a reset empties the data
/// directory, it does not delete it.
fn ensure_within(root: &Path, target: &Path) -> Result<(), ResetError> {
    if target == root
        || !target.starts_with(root)
        || target
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(ResetError::OutsideRoot(target.to_path_buf()));
    }
    Ok(())
}

/// What one removal or measurement covered.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Footprint {
    entries: u64,
    bytes: u64,
}

impl Footprint {
    fn add(self, other: Self) -> Self {
        Self {
            entries: self.entries.saturating_add(other.entries),
            bytes: self.bytes.saturating_add(other.bytes),
        }
    }
}

fn filesystem_error(path: &Path, source: io::Error) -> ResetError {
    ResetError::Filesystem {
        path: path.to_path_buf(),
        source,
    }
}

/// Measures a path without changing it, using the same walk the removal uses.
///
/// Symlinks are counted as the single entries they are and never followed, so
/// a link into a huge tree outside the root contributes its own size only.
fn measure_within(root: &Path, target: &Path) -> Result<Footprint, ResetError> {
    ensure_within(root, target)?;
    let Some(metadata) = optional_symlink_metadata(target)? else {
        return Ok(Footprint::default());
    };
    if !metadata.is_dir() {
        return Ok(Footprint {
            entries: 1,
            bytes: metadata.len(),
        });
    }
    let mut total = Footprint {
        entries: 1,
        bytes: 0,
    };
    for child in read_children(target)? {
        total = total.add(measure_within(root, &child)?);
    }
    Ok(total)
}

/// Removes a path and everything under it, refusing anything outside `root`
/// and never descending through a symlink.
fn remove_within(root: &Path, target: &Path) -> Result<Footprint, ResetError> {
    ensure_within(root, target)?;
    let Some(metadata) = optional_symlink_metadata(target)? else {
        return Ok(Footprint::default());
    };
    if !metadata.is_dir() {
        // A symlink lands here too: unlinking the link never reaches whatever
        // it points at.
        fs::remove_file(target).map_err(|error| filesystem_error(target, error))?;
        return Ok(Footprint {
            entries: 1,
            bytes: metadata.len(),
        });
    }
    let mut total = Footprint {
        entries: 1,
        bytes: 0,
    };
    for child in read_children(target)? {
        total = total.add(remove_within(root, &child)?);
    }
    fs::remove_dir(target).map_err(|error| filesystem_error(target, error))?;
    Ok(total)
}

fn optional_symlink_metadata(path: &Path) -> Result<Option<fs::Metadata>, ResetError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(filesystem_error(path, error)),
    }
}

/// Lists one directory's children as paths built from single components, so a
/// crafted entry name can never widen the walk.
fn read_children(directory: &Path) -> Result<Vec<PathBuf>, ResetError> {
    let entries = fs::read_dir(directory).map_err(|error| filesystem_error(directory, error))?;
    let mut children = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| filesystem_error(directory, error))?;
        let name = entry.file_name();
        let child = directory.join(&name);
        ensure_single_component(&name.to_string_lossy(), &child)?;
        children.push(child);
    }
    children.sort();
    Ok(children)
}

fn item(kind: &str, count: u64, bytes: u64) -> ResetItem {
    ResetItem {
        kind: kind.to_owned(),
        count,
        bytes,
        names: Vec::new(),
    }
}

fn named_item(kind: &str, footprint: Footprint, names: Vec<String>) -> ResetItem {
    ResetItem {
        kind: kind.to_owned(),
        count: footprint.entries,
        bytes: footprint.bytes,
        names,
    }
}

fn footprint_item(kind: &str, footprint: Footprint) -> ResetItem {
    item(kind, footprint.entries, footprint.bytes)
}

fn compose(scope: &str, dry_run: bool, items: Vec<ResetItem>) -> ResetResult {
    let total_items = items
        .iter()
        .fold(0_u64, |total, entry| total.saturating_add(entry.count));
    let total_bytes = items
        .iter()
        .fold(0_u64, |total, entry| total.saturating_add(entry.bytes));
    ResetResult {
        scope: scope.to_owned(),
        dry_run,
        items,
        total_items,
        total_bytes,
    }
}

/// Everything a tier reset needs beyond the store: where state lives, and
/// which credential store holds the caller material.
#[derive(Clone, Debug)]
pub struct ResetContext {
    paths: ResetPaths,
    credentials: CredentialStore,
}

impl ResetContext {
    #[must_use]
    pub const fn new(paths: ResetPaths, credentials: CredentialStore) -> Self {
        Self { paths, credentials }
    }

    #[must_use]
    pub const fn paths(&self) -> &ResetPaths {
        &self.paths
    }
}

/// Runs one reset tier, or forecasts it exactly when `dry_run` is set.
///
/// A dry run reads only: it opens no transaction, removes no file, and
/// touches no credential-store entry. The counts it reports are the counts
/// the matching real run then removes.
///
/// # Errors
///
/// Returns a [`ResetError`] when durable state, the filesystem, or the
/// credential store refuses.
pub async fn run_tier(
    store: &Store,
    context: &ResetContext,
    tier: ResetTier,
    dry_run: bool,
) -> Result<ResetResult, ResetError> {
    let items = match tier {
        ResetTier::Access => access_items(store, dry_run).await?,
        ResetTier::Identity => identity_items(store, context, dry_run).await?,
        ResetTier::History => history_items(store, dry_run).await?,
        ResetTier::Registry => registry_items(store, dry_run).await?,
    };
    Ok(compose(tier.label(), dry_run, items))
}

async fn access_items(store: &Store, dry_run: bool) -> Result<Vec<ResetItem>, ResetError> {
    let tally = if dry_run {
        store.access_reset_tally().await?
    } else {
        store.reset_access().await?
    };
    Ok(vec![
        item("grants", tally.grants, 0),
        item("approvals", tally.approvals, 0),
        item("flow_authorizations", tally.flow_authorizations, 0),
    ])
}

async fn identity_items(
    store: &Store,
    context: &ResetContext,
    dry_run: bool,
) -> Result<Vec<ResetItem>, ResetError> {
    let registrations = store.list_callers().await?;
    // Every registered caller has a credential-store entry, revoked or not: a
    // file-level wipe that skipped them would leave orphaned keychain items
    // behind, which is exactly the failure this tier exists to avoid.
    let locators = registrations
        .iter()
        .filter_map(|registration| SecretLocator::for_caller(&registration.caller_id).ok())
        .collect::<Vec<_>>();
    let active = registrations
        .iter()
        .filter(|registration| registration.revoked_at_ms.is_none())
        .map(|registration| registration.caller_id.clone())
        .collect::<Vec<_>>();

    let callers_directory = context.paths.child(CALLERS_DIRECTORY)?;
    let root = context.paths.root().to_path_buf();

    if dry_run {
        let present = present_credentials(&context.credentials, locators).await?;
        let files = measure_within(&root, &callers_directory)?;
        return Ok(vec![
            item("callers", u64::try_from(active.len()).unwrap_or(0), 0),
            footprint_item("caller_files", files),
            item("keychain_entries", present, 0),
        ]);
    }

    let now_ms = now_ms();
    let mut revoked = 0_u64;
    for caller_id in active {
        store.revoke_caller(caller_id, now_ms).await?;
        revoked = revoked.saturating_add(1);
    }
    let purged = purge_credentials(&context.credentials, locators).await?;
    let files = remove_within(&root, &callers_directory)?;
    Ok(vec![
        item("callers", revoked, 0),
        footprint_item("caller_files", files),
        item("keychain_entries", purged, 0),
    ])
}

async fn history_items(store: &Store, dry_run: bool) -> Result<Vec<ResetItem>, ResetError> {
    // The evidence tally is read first either way, so the real run reports the
    // same bytes the dry run forecast: the purge itself only returns handle
    // counts, never the blob bytes it freed.
    let evidence = store.evidence_reset_tally().await?;
    let history = if dry_run {
        store.history_reset_tally().await?
    } else {
        store.reset_history().await?
    };
    let evidence = if dry_run {
        evidence
    } else {
        let mut handles = 0_u64;
        for _ in 0..EVIDENCE_PURGE_ROUNDS {
            let outcome = store.reset_evidence(EVIDENCE_PURGE_PAGE).await?;
            handles = handles.saturating_add(u64::from(outcome.handles_deleted));
            if !outcome.has_more {
                break;
            }
        }
        ResetTally {
            count: handles,
            bytes: evidence.bytes,
        }
    };
    Ok(vec![
        item(
            "audit_events",
            history.audit_events.count,
            history.audit_events.bytes,
        ),
        item("evidence", evidence.count, evidence.bytes),
        item(
            "flow_runs",
            history.flow_runs.count,
            history.flow_runs.bytes,
        ),
    ])
}

async fn registry_items(store: &Store, dry_run: bool) -> Result<Vec<ResetItem>, ResetError> {
    let tally = if dry_run {
        store.registry_reset_tally().await?
    } else {
        store.reset_registry().await?
    };
    // Bytes stay zero: unregistering a model never touches its weights.
    Ok(vec![item("models", tally.count, 0)])
}

/// Counts the credential-store entries a purge would remove, reading only.
async fn present_credentials(
    credentials: &CredentialStore,
    locators: Vec<SecretLocator>,
) -> Result<u64, ResetError> {
    with_credentials(credentials, locators, |backend, locators| {
        let mut present = 0_u64;
        for locator in locators {
            if backend.get(locator)?.is_some() {
                present = present.saturating_add(1);
            }
        }
        Ok(present)
    })
    .await
}

/// Removes each caller's credential-store entry, counting the ones that were
/// actually there.
async fn purge_credentials(
    credentials: &CredentialStore,
    locators: Vec<SecretLocator>,
) -> Result<u64, ResetError> {
    with_credentials(credentials, locators, |backend, locators| {
        let mut removed = 0_u64;
        for locator in locators {
            if backend.delete(locator)? {
                removed = removed.saturating_add(1);
            }
        }
        Ok(removed)
    })
    .await
}

/// Runs one bounded credential-store operation off the async runtime, since
/// native credential stores block.
async fn with_credentials<F>(
    credentials: &CredentialStore,
    locators: Vec<SecretLocator>,
    operation: F,
) -> Result<u64, ResetError>
where
    F: FnOnce(&dyn SecretBackend, &[SecretLocator]) -> Result<u64, SecretBackendError>
        + Send
        + 'static,
{
    if locators.is_empty() {
        return Ok(0);
    }
    let credentials = credentials.clone();
    tokio::task::spawn_blocking(move || match &credentials {
        CredentialStore::Injected(backend) => operation(backend.as_ref(), &locators),
        CredentialStore::Native => {
            let backend = NativeSecretBackend::new()?;
            operation(&backend, &locators)
        }
    })
    .await
    .map_err(|_| ResetError::Interrupted)?
    .map_err(ResetError::Credentials)
}

/// What a factory reset covers beyond the four tiers.
#[derive(Clone, Debug, Default)]
pub struct FactoryResetOptions {
    /// Also delete the weights of every registered model. Off by default:
    /// weights are multi-gigabyte, live outside the data directory, and are
    /// re-downloadable, so they are never swept in with the state.
    pub include_weights: bool,
}

/// The durable record a factory reset leaves behind.
#[derive(Clone, Debug)]
pub struct FactoryReceipt {
    /// Where the receipt was written: beside the data root, never inside it.
    pub path: PathBuf,
    /// Everything that went, in counts and bytes.
    pub result: ResetResult,
    /// The audit event this reset recorded before wiping the ledger.
    pub audit_event_id: String,
}

/// Forecasts a factory reset exactly, changing nothing.
///
/// Opens the durable store read-only for its counts, so it requires the
/// daemon to be stopped for the same reason the reset itself does.
///
/// # Errors
///
/// Returns a [`ResetError`] when durable state or the filesystem refuses.
pub async fn preview_factory_reset(
    context: &ResetContext,
    options: &FactoryResetOptions,
) -> Result<ResetResult, ResetError> {
    let store = Store::open(context.paths.state_path())?;
    let outcome = factory_items(&store, context, options).await;
    let shutdown = store.shutdown().await;
    let items = outcome?;
    shutdown?;
    Ok(compose("factory", true, items))
}

/// Performs a factory reset: every tier, settings back to defaults, the
/// authored flow library, and everything else under the data root.
///
/// The audit event is written -- and read back -- before a single byte is
/// removed, so a reset that cannot record itself never happens at all. The
/// receipt lands beside the data root rather than inside it, which is the
/// only record that survives the wipe.
///
/// # Errors
///
/// Returns [`ResetError::DaemonRunning`] when a daemon still owns the store,
/// or another [`ResetError`] when durable state, the credential store, or the
/// filesystem refuses.
pub async fn run_factory_reset(
    context: &ResetContext,
    options: &FactoryResetOptions,
    caller_id: &CallerId,
    endpoint: &LocalEndpoint,
) -> Result<FactoryReceipt, ResetError> {
    if daemon_owns_store(endpoint) {
        return Err(ResetError::DaemonRunning);
    }
    let store = Store::open(context.paths.state_path())?;
    let prepared = prepare_factory_reset(&store, context, options, caller_id).await;
    let shutdown = store.shutdown().await;
    let (items, audit_event_id, weight_paths) = prepared?;
    shutdown?;

    let purged = wipe(context, &weight_paths).await?;
    // The identity tier already contributes a `keychain_entries` item; the
    // wipe replaces its forecast with what it actually removed rather than
    // adding a second entry that would double the totals.
    let mut items = items;
    if let Some(entry) = items
        .iter_mut()
        .find(|entry| entry.kind == "keychain_entries")
    {
        entry.count = purged;
    }
    let result = compose("factory", false, items);
    let path = write_receipt(context, &result, &audit_event_id, &weight_paths)?;
    Ok(FactoryReceipt {
        path,
        result,
        audit_event_id,
    })
}

/// Counts the reset, records it in the ledger, and proves the record landed --
/// all before anything is removed.
async fn prepare_factory_reset(
    store: &Store,
    context: &ResetContext,
    options: &FactoryResetOptions,
    caller_id: &CallerId,
) -> Result<(Vec<ResetItem>, String, Vec<PathBuf>), ResetError> {
    let items = factory_items(store, context, options).await?;
    let weight_paths = if options.include_weights {
        registered_weight_paths(store).await?
    } else {
        Vec::new()
    };
    let forecast = compose("factory", true, items.clone());
    let audit_event_id = append_factory_audit(store, caller_id, &forecast).await?;
    confirm_audit_recorded(store, &audit_event_id).await?;
    Ok((items, audit_event_id, weight_paths))
}

/// The complete factory forecast: every tier's own items, plus a partition of
/// the data root that deliberately leaves out the directories a tier already
/// accounts for, so nothing is counted twice.
async fn factory_items(
    store: &Store,
    context: &ResetContext,
    options: &FactoryResetOptions,
) -> Result<Vec<ResetItem>, ResetError> {
    let mut items = Vec::new();
    for tier in ResetTier::all() {
        // Always the forecast: a factory reset deletes the database whole, so
        // running the tiers' own deletes first would only slow it down.
        items.extend(run_tier(store, context, tier, true).await?.items);
    }
    let root = context.paths.root().to_path_buf();
    let flows = context.paths.flow_library()?;
    // Counted by authored definition rather than by filesystem node: the
    // number the typed confirmation needs is "how many flows".
    let flow_names = flow_names(&flows)?;
    let flow_bytes = measure_within(&root, &flows)?.bytes;
    items.push(named_item(
        "flows",
        Footprint {
            entries: u64::try_from(flow_names.len()).unwrap_or(u64::MAX),
            bytes: flow_bytes,
        },
        flow_names,
    ));
    items.push(footprint_item(
        "settings",
        measure_within(&root, &context.paths.child(SETTINGS_FILE)?)?,
    ));
    items.push(footprint_item(
        "logs",
        measure_within(&root, &context.paths.child(LOGS_DIRECTORY)?)?,
    ));
    items.push(footprint_item(
        "runtime",
        measure_within(&root, &context.paths.child(RUNTIME_DIRECTORY)?)?,
    ));
    items.push(footprint_item("state_database", state_footprint(context)?));
    items.push(footprint_item(
        "other_data_files",
        remaining_footprint(context)?,
    ));
    if options.include_weights {
        let (count, bytes) = weight_footprint(store).await?;
        items.push(item("model_weights", count, bytes));
    }
    // The evidence tier already carries the blob bytes, so the filesystem
    // partition above deliberately leaves the evidence directory out.
    Ok(items)
}

/// The state database and its journal siblings, which share its stem.
fn state_footprint(context: &ResetContext) -> Result<Footprint, ResetError> {
    let root = context.paths.root().to_path_buf();
    let mut total = Footprint::default();
    for child in read_children_if_present(&root)? {
        let is_state = child
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(STATE_FILE));
        if is_state {
            total = total.add(measure_within(&root, &child)?);
        }
    }
    Ok(total)
}

/// Everything directly under the root that no other item already counts.
fn remaining_footprint(context: &ResetContext) -> Result<Footprint, ResetError> {
    let root = context.paths.root().to_path_buf();
    let accounted = [
        EVIDENCE_DIRECTORY,
        CALLERS_DIRECTORY,
        LOGS_DIRECTORY,
        RUNTIME_DIRECTORY,
        SETTINGS_FILE,
        FLOW_LIBRARY[0],
    ];
    let mut total = Footprint::default();
    for child in read_children_if_present(&root)? {
        let Some(name) = child.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if accounted.contains(&name) || name.starts_with(STATE_FILE) {
            continue;
        }
        total = total.add(measure_within(&root, &child)?);
    }
    Ok(total)
}

fn read_children_if_present(directory: &Path) -> Result<Vec<PathBuf>, ResetError> {
    if optional_symlink_metadata(directory)?.is_none() {
        return Ok(Vec::new());
    }
    read_children(directory)
}

/// The authored flow definitions a factory reset removes, by name, so the
/// typed confirmation is informed about what it is destroying.
fn flow_names(flows: &Path) -> Result<Vec<String>, ResetError> {
    if optional_symlink_metadata(flows)?.is_none() {
        return Ok(Vec::new());
    }
    let mut names = read_children(flows)?
        .iter()
        .filter_map(|path| path.file_name().and_then(|name| name.to_str()))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    names.sort();
    Ok(names)
}

async fn registered_weight_paths(store: &Store) -> Result<Vec<PathBuf>, ResetError> {
    Ok(store
        .list_models()
        .await?
        .iter()
        .map(|model| model.path.clone())
        .collect())
}

/// Weights are counted from the registry rather than by sweeping a directory:
/// the models directory is owner-configurable, may sit anywhere, and may hold
/// files PAM never registered. Only artifacts PAM itself recorded are ever
/// removed.
async fn weight_footprint(store: &Store) -> Result<(u64, u64), ResetError> {
    let mut count = 0_u64;
    let mut bytes = 0_u64;
    for path in registered_weight_paths(store).await? {
        let Some(metadata) = optional_symlink_metadata(&path)? else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        count = count.saturating_add(1);
        bytes = bytes.saturating_add(metadata.len());
    }
    Ok((count, bytes))
}

pub(super) async fn append_factory_audit(
    store: &Store,
    caller_id: &CallerId,
    forecast: &ResetResult,
) -> Result<String, ResetError> {
    let occurred_at_ms = now_ms();
    let event_id = format!("reset-factory-{occurred_at_ms}-{}", caller_id.as_str());
    let detail = format!(
        "scope=factory items={} bytes={}",
        forecast.total_items, forecast.total_bytes
    );
    let record = store
        .append_audit_event(AppendAuditEvent {
            event_id: event_id.clone(),
            project_id: ProjectId::daemon_scope(),
            caller_id: caller_id.clone(),
            action: "reset.factory".to_owned(),
            decision: "allow".to_owned(),
            outcome: "changed".to_owned(),
            redacted_detail: redact_audit_detail(detail.as_bytes()),
            occurred_at_ms,
            retain_until_ms: occurred_at_ms
                .saturating_add(u64::try_from(AUDIT_RETENTION.as_millis()).unwrap_or(u64::MAX)),
        })
        .await?;
    Ok(record.event_id)
}

/// Reads the reset's own audit event straight back. A reset that cannot prove
/// it recorded itself must not proceed to erase the ledger that would have
/// held the record.
pub(super) async fn confirm_audit_recorded(
    store: &Store,
    event_id: &str,
) -> Result<(), ResetError> {
    let recent = store.recent_audit_events(AUDIT_READBACK_LIMIT).await?;
    if recent.events.iter().any(|event| event.event_id == event_id) {
        return Ok(());
    }
    Err(ResetError::Store(StoreError::InvalidState(
        "the factory reset could not record itself in the audit ledger".to_owned(),
    )))
}

/// Empties the data root and purges the credential store, leaving the root
/// directory itself in place for the next daemon to reopen.
async fn wipe(context: &ResetContext, weight_paths: &[PathBuf]) -> Result<u64, ResetError> {
    let root = context.paths.root().to_path_buf();
    let locators = credential_locators_from_files(context)?;
    let purged = purge_credentials(&context.credentials, locators).await?;
    for child in read_children_if_present(&root)? {
        remove_within(&root, &child)?;
    }
    for path in weight_paths {
        remove_registered_weight(path)?;
    }
    Ok(purged)
}

/// Rebuilds each caller's credential locator from the identity files the data
/// root still holds, so the keychain purge happens before the files that name
/// the callers are removed.
fn credential_locators_from_files(
    context: &ResetContext,
) -> Result<Vec<SecretLocator>, ResetError> {
    let callers = context.paths.child(CALLERS_DIRECTORY)?;
    let mut locators = Vec::new();
    for path in read_children_if_present(&callers)? {
        let Ok(contents) = fs::read_to_string(&path) else {
            continue;
        };
        let Some(caller_id) = parse_caller_id(&contents) else {
            continue;
        };
        if let Ok(locator) = SecretLocator::for_caller(&caller_id) {
            locators.push(locator);
        }
    }
    Ok(locators)
}

/// Reads the one `caller_id = "..."` line PAM writes into an identity file.
fn parse_caller_id(contents: &str) -> Option<CallerId> {
    contents
        .lines()
        .filter_map(|line| line.trim().strip_prefix("caller_id"))
        .filter_map(|rest| rest.trim_start().strip_prefix('='))
        .filter_map(|rest| {
            let trimmed = rest.trim();
            trimmed
                .strip_prefix('"')
                .and_then(|rest| rest.strip_suffix('"'))
        })
        .map(CallerId::new)
        .next()
}

/// Removes one registered weight file, and only if it really is a file. Never
/// a directory, never a symlink's target.
fn remove_registered_weight(path: &Path) -> Result<(), ResetError> {
    let Some(metadata) = optional_symlink_metadata(path)? else {
        return Ok(());
    };
    if !metadata.is_file() {
        return Ok(());
    }
    fs::remove_file(path).map_err(|error| filesystem_error(path, error))
}

fn write_receipt(
    context: &ResetContext,
    result: &ResetResult,
    audit_event_id: &str,
    weight_paths: &[PathBuf],
) -> Result<PathBuf, ResetError> {
    let now_ms = now_ms();
    let path = context.paths.receipt_path(now_ms)?;
    let mut body = String::from("PAM factory reset receipt\n");
    let write = |body: &mut String, line: fmt::Arguments<'_>| {
        body.write_fmt(line)
            .expect("writing to a String cannot fail");
    };
    write(&mut body, format_args!("occurred_at_unix_ms={now_ms}\n"));
    write(&mut body, format_args!("audit_event_id={audit_event_id}\n"));
    write(
        &mut body,
        format_args!("data_directory={}\n", context.paths.root().display()),
    );
    write(
        &mut body,
        format_args!(
            "total_items={} total_bytes={}\n",
            result.total_items, result.total_bytes
        ),
    );
    body.push_str("removed:\n");
    for entry in &result.items {
        write(
            &mut body,
            format_args!(
                "  {} count={} bytes={}\n",
                entry.kind, entry.count, entry.bytes
            ),
        );
        for name in &entry.names {
            write(&mut body, format_args!("    {name}\n"));
        }
    }
    if weight_paths.is_empty() {
        body.push_str("model_weights: kept\n");
    } else {
        body.push_str("model_weights:\n");
        for weight in weight_paths {
            write(&mut body, format_args!("  {}\n", weight.display()));
        }
    }
    fs::write(&path, body).map_err(|error| filesystem_error(&path, error))?;
    Ok(path)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}
