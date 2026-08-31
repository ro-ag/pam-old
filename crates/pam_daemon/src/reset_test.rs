use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use pam_core::{
    ApprovalId, CallerCredential, CallerId, ContentDigest, EvidenceHandle, GrantId, IdempotencyKey,
    ProjectId, RequestId,
};
use pam_model::{GgufMetadata, LicenseSnapshot, ModelKey, ModelSource};
use pam_platform::{LocalEndpoint, SecretBackend, SecretLocator};
use pam_policy::{ApprovalRequirement, CapabilityName, Effect, Grant, ResourceName, ResourceScope};
use pam_protocol::{ResetResult, ResetTier};
use pam_store::{
    AcceptRequest, AppendAuditEvent, AuthorizationRequest, EvidenceRedaction, EvidenceRetention,
    PutEvidence, PutGrant, RegisteredModel, Store,
};

use crate::connectors_test::MemorySecretBackend;
use crate::reset::{
    CredentialStore, FactoryResetOptions, ResetContext, ResetError, ResetPaths,
    append_factory_audit, confirm_audit_recorded, daemon_owns_store, preview_factory_reset,
    run_factory_reset, run_tier,
};

const NOW_MS: u64 = 1_700_000_000_000;
const APPROVAL_TTL_MS: u64 = 300_000;
/// At least `ModelDescriptor::MIN_SIZE_BYTES`, so the registry accepts it.
const WEIGHT_BYTES: &[u8] = b"pretend gguf weights, padded out";

static SCRATCH_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A throwaway data root that behaves like the platform one: the state
/// database sits directly inside it, so [`ResetPaths::for_state_path`]
/// resolves the same layout the daemon resolves in production.
struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(name: &str) -> Self {
        let sequence = SCRATCH_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "pam-reset-{name}-{}-{sequence}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("data")).expect("scratch root must be creatable");
        Self { root }
    }

    /// A path inside the data root the reset resolves.
    fn path(&self, relative: &str) -> PathBuf {
        self.root.join("data").join(relative)
    }

    /// A path beside the data root: model weights normally live outside PAM's
    /// data directory, and so does anything a reset must never reach.
    fn outside(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    fn write(&self, relative: &str, contents: &[u8]) -> PathBuf {
        let path = self.path(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("scratch parent must be creatable");
        }
        fs::write(&path, contents).expect("scratch file must be writable");
        path
    }

    fn write_outside(&self, relative: &str, contents: &[u8]) -> PathBuf {
        let path = self.outside(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("scratch parent must be creatable");
        }
        fs::write(&path, contents).expect("scratch file must be writable");
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn caller(kind: &str) -> CallerId {
    CallerId::new(format!("caller-{kind}"))
}

fn project() -> ProjectId {
    ProjectId::new("project-reset")
}

fn capability() -> CapabilityName {
    CapabilityName::parse("model.infer").expect("static capability is valid")
}

fn resource() -> ResourceName {
    ResourceName::parse("model:vendor/name").expect("static resource is valid")
}

fn item(result: &ResetResult, kind: &str) -> u64 {
    result
        .items
        .iter()
        .find(|entry| entry.kind == kind)
        .unwrap_or_else(|| panic!("reset result must report {kind}: {:?}", result.items))
        .count
}

fn item_bytes(result: &ResetResult, kind: &str) -> u64 {
    result
        .items
        .iter()
        .find(|entry| entry.kind == kind)
        .unwrap_or_else(|| panic!("reset result must report {kind}"))
        .bytes
}

fn registered_model(name: &str, path: &Path) -> RegisteredModel {
    RegisteredModel {
        key: ModelKey::new("vendor", name).expect("static model key is valid"),
        path: path.to_path_buf(),
        digest: ContentDigest::parse(format!("sha256:{}", "a".repeat(64)))
            .expect("static digest is valid"),
        size_bytes: WEIGHT_BYTES.len() as u64,
        gguf: GgufMetadata {
            version: 3,
            tensor_count: 1,
            metadata_kv_count: 1,
            architecture: None,
            model_name: None,
            license: None,
        },
        license: LicenseSnapshot::new(
            "apache-2.0".to_owned(),
            "https://example.invalid/license".to_owned(),
            ContentDigest::parse(format!("sha256:{}", "b".repeat(64)))
                .expect("static digest is valid"),
        )
        .expect("static license is valid"),
        source: ModelSource::Local,
        registered_at_ms: NOW_MS,
    }
}

/// Fills one scratch root with a little of every kind of state a reset tier
/// covers, so a tier that reached outside its own scope would be visible.
#[allow(clippy::too_many_lines)] // One seed per state class, kept in one readable place.
async fn seeded(scratch: &Scratch) -> (Store, ResetContext, Arc<MemorySecretBackend>) {
    let store = Store::open(scratch.path("state.sqlite3")).expect("store must open");

    let cli = caller("cli");
    let gui = caller("gui");
    for (caller_id, kind) in [(cli.clone(), "cli"), (gui.clone(), "gui")] {
        store
            .register_caller_with_kind(
                caller_id,
                CallerCredential::new(format!("pam_{kind}_credential")),
                Some(kind.to_owned()),
                NOW_MS,
            )
            .await
            .expect("caller registration must succeed");
    }

    // A project row has to exist before a grant can reference it.
    store
        .accept(
            AcceptRequest {
                request_id: RequestId::new("request-seed"),
                caller_id: cli.clone(),
                project_id: project(),
                idempotency_key: IdempotencyKey::new("idempotency-seed"),
                operation_kind: "status".to_owned(),
                operation: b"seed".to_vec(),
            },
            NOW_MS,
        )
        .await
        .expect("seed request must be accepted");

    store
        .put_grant(PutGrant {
            grant: Grant {
                id: GrantId::new("grant-approval-required"),
                caller: cli.clone(),
                project: project(),
                capability: capability(),
                resource: ResourceScope::Any,
                effect: Effect::Allow,
                approval: ApprovalRequirement::Once,
                expires_at_ms: None,
                revoked_at_ms: None,
            },
            created_at_ms: NOW_MS,
        })
        .await
        .expect("grant must be stored");

    // Evaluating an approval-required grant is what mints an approval row.
    store
        .authorize(
            AuthorizationRequest {
                caller_id: cli.clone(),
                project_id: project(),
                capability: capability(),
                resource: resource(),
                approval_id: None::<ApprovalId>,
            },
            NOW_MS,
            APPROVAL_TTL_MS,
        )
        .await
        .expect("authorization must evaluate");

    store
        .append_audit_event(AppendAuditEvent {
            event_id: "audit-seed".to_owned(),
            project_id: project(),
            caller_id: cli.clone(),
            action: "seed".to_owned(),
            decision: "allow".to_owned(),
            outcome: "changed".to_owned(),
            redacted_detail: "seeded audit detail".to_owned(),
            occurred_at_ms: NOW_MS,
            retain_until_ms: NOW_MS + 1,
        })
        .await
        .expect("audit event must append");

    store
        .put_evidence(
            PutEvidence {
                handle: EvidenceHandle::parse("evidence://reset/seed")
                    .expect("static evidence handle is valid"),
                project_id: project(),
                media_type: "text/plain".to_owned(),
                retention: EvidenceRetention::Session,
                redaction: EvidenceRedaction::Unredacted,
                bytes: b"evidence bytes".to_vec(),
            },
            NOW_MS,
        )
        .await
        .expect("evidence must be stored");

    let weights = scratch.write_outside("weights/vendor-name.gguf", WEIGHT_BYTES);
    store
        .put_model(registered_model("name", &weights))
        .await
        .expect("model must register");

    scratch.write(
        "callers/cli.toml",
        format!("version = 1\ncaller_id = \"{}\"\n", cli.as_str()).as_bytes(),
    );
    scratch.write(
        "callers/gui.toml",
        format!("version = 1\ncaller_id = \"{}\"\n", gui.as_str()).as_bytes(),
    );
    scratch.write(".pam/flows/release-readiness.toml", b"name = \"release\"\n");
    scratch.write(".pam/flows/worktree-triage.toml", b"name = \"triage\"\n");
    scratch.write("settings.json", b"{\"modelsDir\":\"/tmp/models\"}");
    scratch.write("logs/daemon.log", b"log line\n");
    scratch.write("runtime/daemon.lock", b"");

    let backend = Arc::new(MemorySecretBackend::default());
    for caller_id in [cli, gui] {
        let locator = SecretLocator::for_caller(&caller_id).expect("locator must derive");
        backend
            .set(&locator, &CallerCredential::new("pam_seeded_credential"))
            .expect("memory backend must accept a secret");
    }

    let context = ResetContext::new(
        ResetPaths::for_state_path(&scratch.path("state.sqlite3")).expect("root must resolve"),
        CredentialStore::Injected(Arc::clone(&backend) as _),
    );
    (store, context, backend)
}

async fn tier(
    store: &Store,
    context: &ResetContext,
    tier: ResetTier,
    dry_run: bool,
) -> ResetResult {
    run_tier(store, context, tier, dry_run)
        .await
        .expect("reset tier must succeed")
}

/// The forecast a factory reset records in its audit line, built from the
/// tiers exactly the way the reset itself builds it.
async fn preview_tier_totals(store: &Store, context: &ResetContext) -> ResetResult {
    let mut items = Vec::new();
    for scope in ResetTier::all() {
        items.extend(tier(store, context, scope, true).await.items);
    }
    let total_items = items.iter().map(|entry| entry.count).sum();
    let total_bytes = items.iter().map(|entry| entry.bytes).sum();
    ResetResult {
        scope: "factory".to_owned(),
        dry_run: true,
        items,
        total_items,
        total_bytes,
    }
}

fn credential_present(backend: &MemorySecretBackend, kind: &str) -> bool {
    let locator = SecretLocator::for_caller(&caller(kind)).expect("locator must derive");
    backend
        .get(&locator)
        .expect("memory backend must answer")
        .is_some()
}

#[tokio::test]
async fn access_dry_run_forecasts_exactly_what_the_real_run_removes() {
    let scratch = Scratch::new("access");
    let (store, context, _backend) = seeded(&scratch).await;

    let forecast = tier(&store, &context, ResetTier::Access, true).await;
    assert!(forecast.dry_run);
    assert_eq!(item(&forecast, "grants"), 1);
    assert_eq!(item(&forecast, "approvals"), 1);

    // A dry run must change nothing, so a second forecast is identical.
    let repeated = tier(&store, &context, ResetTier::Access, true).await;
    assert_eq!(repeated.items, forecast.items);

    let applied = tier(&store, &context, ResetTier::Access, false).await;
    assert!(!applied.dry_run);
    assert_eq!(applied.items, forecast.items);

    let after = tier(&store, &context, ResetTier::Access, true).await;
    assert_eq!(after.total_items, 0);
    store.shutdown().await.expect("store must shut down");
}

#[tokio::test]
async fn access_reset_leaves_every_other_tier_untouched() {
    let scratch = Scratch::new("access-scope");
    let (store, context, backend) = seeded(&scratch).await;

    tier(&store, &context, ResetTier::Access, false).await;

    assert_eq!(
        item(
            &tier(&store, &context, ResetTier::Registry, true).await,
            "models"
        ),
        1
    );
    assert_eq!(
        item(
            &tier(&store, &context, ResetTier::History, true).await,
            "evidence"
        ),
        1
    );
    let identity = tier(&store, &context, ResetTier::Identity, true).await;
    assert_eq!(item(&identity, "callers"), 2);
    assert_eq!(item(&identity, "keychain_entries"), 2);
    assert!(credential_present(&backend, "cli"));
    assert!(scratch.path("callers/cli.toml").exists());
    store.shutdown().await.expect("store must shut down");
}

#[tokio::test]
async fn identity_reset_purges_caller_files_and_their_keychain_entries() {
    let scratch = Scratch::new("identity");
    let (store, context, backend) = seeded(&scratch).await;

    let forecast = tier(&store, &context, ResetTier::Identity, true).await;
    assert_eq!(item(&forecast, "callers"), 2);
    assert_eq!(item(&forecast, "keychain_entries"), 2);
    // The forecast must not have touched a single credential-store entry.
    assert!(credential_present(&backend, "cli"));
    assert!(credential_present(&backend, "gui"));

    let applied = tier(&store, &context, ResetTier::Identity, false).await;
    assert_eq!(item(&applied, "callers"), 2);
    assert_eq!(item(&applied, "keychain_entries"), 2);
    assert_eq!(
        item(&applied, "caller_files"),
        item(&forecast, "caller_files")
    );

    assert!(
        !credential_present(&backend, "cli"),
        "the cli caller's keychain entry must be gone, not just its file"
    );
    assert!(!credential_present(&backend, "gui"));
    assert!(!scratch.path("callers").exists());
    assert!(
        store
            .list_callers()
            .await
            .expect("callers must list")
            .iter()
            .all(|registration| registration.revoked_at_ms.is_some())
    );
    // Identity is its own tier: the registry and the ledger are untouched.
    assert_eq!(
        item(
            &tier(&store, &context, ResetTier::Registry, true).await,
            "models"
        ),
        1
    );
    store.shutdown().await.expect("store must shut down");
}

#[tokio::test]
async fn history_reset_clears_the_ledger_and_evidence_and_nothing_else() {
    let scratch = Scratch::new("history");
    let (store, context, _backend) = seeded(&scratch).await;

    let forecast = tier(&store, &context, ResetTier::History, true).await;
    assert_eq!(item(&forecast, "evidence"), 1);
    assert!(item(&forecast, "audit_events") > 0);
    assert!(item_bytes(&forecast, "evidence") > 0);

    let applied = tier(&store, &context, ResetTier::History, false).await;
    assert_eq!(item(&applied, "evidence"), item(&forecast, "evidence"));
    assert_eq!(
        item_bytes(&applied, "evidence"),
        item_bytes(&forecast, "evidence")
    );

    let after = tier(&store, &context, ResetTier::History, true).await;
    assert_eq!(item(&after, "evidence"), 0);
    assert_eq!(item(&after, "audit_events"), 0);
    assert_eq!(
        item(
            &tier(&store, &context, ResetTier::Access, true).await,
            "grants"
        ),
        1
    );
    assert_eq!(
        item(
            &tier(&store, &context, ResetTier::Registry, true).await,
            "models"
        ),
        1
    );
    store.shutdown().await.expect("store must shut down");
}

#[tokio::test]
async fn registry_reset_unregisters_models_and_leaves_the_weights_on_disk() {
    let scratch = Scratch::new("registry");
    let (store, context, _backend) = seeded(&scratch).await;
    let weights = scratch.outside("weights/vendor-name.gguf");

    let forecast = tier(&store, &context, ResetTier::Registry, true).await;
    assert_eq!(item(&forecast, "models"), 1);
    assert_eq!(item_bytes(&forecast, "models"), 0);

    let applied = tier(&store, &context, ResetTier::Registry, false).await;
    assert_eq!(item(&applied, "models"), 1);
    assert!(
        store
            .list_models()
            .await
            .expect("models must list")
            .is_empty(),
        "every model must be unregistered"
    );
    assert!(
        weights.exists(),
        "unregistering a model must never delete its weights"
    );
    store.shutdown().await.expect("store must shut down");
}

#[tokio::test]
async fn factory_reset_refuses_while_a_daemon_owns_the_store() {
    let scratch = Scratch::new("factory-running");
    let (store, context, _backend) = seeded(&scratch).await;
    store.shutdown().await.expect("store must shut down");

    let runtime = scratch.outside("held-runtime");
    fs::create_dir_all(&runtime).expect("runtime directory must be creatable");
    let endpoint = LocalEndpoint::ipc(runtime);
    let lock = fs::File::create(endpoint.ownership_path()).expect("lock file must be creatable");
    lock.lock().expect("test must be able to hold the lock");
    assert!(daemon_owns_store(&endpoint));

    let error = run_factory_reset(
        &context,
        &FactoryResetOptions::default(),
        &caller("cli"),
        &endpoint,
    )
    .await
    .expect_err("a running daemon must block a factory reset");
    assert!(matches!(error, ResetError::DaemonRunning));
    assert!(
        error
            .recovery()
            .expect("a refusal must carry a recovery line")
            .contains("Stop PAM first")
    );
    assert!(
        scratch.path("state.sqlite3").exists(),
        "a refused factory reset must change nothing"
    );
    drop(lock);
}

#[tokio::test]
async fn factory_reset_records_itself_then_wipes_and_leaves_a_readable_receipt() {
    let scratch = Scratch::new("factory");
    let (store, context, backend) = seeded(&scratch).await;
    store.shutdown().await.expect("store must shut down");

    let runtime = scratch.outside("free-runtime");
    fs::create_dir_all(&runtime).expect("runtime directory must be creatable");
    let endpoint = LocalEndpoint::ipc(runtime);

    let forecast = preview_factory_reset(&context, &FactoryResetOptions::default())
        .await
        .expect("factory preview must succeed");
    assert!(forecast.dry_run);
    assert_eq!(item(&forecast, "flows"), 2, "the two authored flows");
    let flows = forecast
        .items
        .iter()
        .find(|entry| entry.kind == "flows")
        .expect("the forecast must name the flow library");
    assert!(flows.names.contains(&"release-readiness.toml".to_owned()));
    // Nothing may be counted twice: totals are what the confirmation reads.
    let mut kinds = forecast
        .items
        .iter()
        .map(|entry| entry.kind.clone())
        .collect::<Vec<_>>();
    let unique = kinds.len();
    kinds.sort();
    kinds.dedup();
    assert_eq!(kinds.len(), unique, "each class appears exactly once");
    assert_eq!(
        forecast.total_items,
        forecast.items.iter().map(|entry| entry.count).sum::<u64>()
    );
    assert!(
        scratch.path(".pam/flows/release-readiness.toml").exists(),
        "a preview must not delete a flow"
    );

    let receipt = run_factory_reset(
        &context,
        &FactoryResetOptions::default(),
        &caller("cli"),
        &endpoint,
    )
    .await
    .expect("factory reset must succeed");

    assert!(!receipt.result.dry_run);
    assert!(
        !receipt.path.starts_with(context.paths().root()),
        "the receipt must live outside the directory the wipe empties"
    );
    let body = fs::read_to_string(&receipt.path).expect("the receipt must be readable afterwards");
    assert!(body.contains(&receipt.audit_event_id));
    assert!(body.contains("release-readiness.toml"));
    assert!(body.contains("model_weights: kept"));

    assert!(
        !scratch.path(".pam/flows").exists(),
        "factory means the flows go too"
    );
    assert!(!scratch.path("state.sqlite3").exists());
    assert!(!scratch.path("settings.json").exists());
    assert!(!scratch.path("logs").exists());
    assert!(
        scratch.outside("weights/vendor-name.gguf").exists(),
        "weights stay unless they are opted in"
    );
    assert!(
        context.paths().root().exists(),
        "the data root itself survives so the next daemon can reopen it"
    );
    assert!(!credential_present(&backend, "cli"));
    assert!(!credential_present(&backend, "gui"));
    let _ = fs::remove_file(&receipt.path);
}

#[tokio::test]
async fn factory_reset_deletes_registered_weights_only_when_they_are_opted_in() {
    let scratch = Scratch::new("factory-weights");
    let (store, context, _backend) = seeded(&scratch).await;
    store.shutdown().await.expect("store must shut down");
    let weights = scratch.outside("weights/vendor-name.gguf");

    let runtime = scratch.outside("free-runtime");
    fs::create_dir_all(&runtime).expect("runtime directory must be creatable");
    let endpoint = LocalEndpoint::ipc(runtime);

    let options = FactoryResetOptions {
        include_weights: true,
    };
    let forecast = preview_factory_reset(&context, &options)
        .await
        .expect("factory preview must succeed");
    assert_eq!(item(&forecast, "model_weights"), 1);
    assert_eq!(
        item_bytes(&forecast, "model_weights"),
        WEIGHT_BYTES.len() as u64
    );
    assert!(weights.exists(), "a preview never deletes a weight");

    let receipt = run_factory_reset(&context, &options, &caller("cli"), &endpoint)
        .await
        .expect("factory reset must succeed");
    assert!(!weights.exists(), "opted-in weights must go");
    let body = fs::read_to_string(&receipt.path).expect("the receipt must be readable");
    assert!(body.contains("vendor-name.gguf"));
    let _ = fs::remove_file(&receipt.path);
}

#[tokio::test]
async fn a_reset_never_reaches_outside_its_resolved_data_root() {
    let scratch = Scratch::new("path-safety");
    let outside = scratch.outside("outside-witness.txt");
    fs::write(&outside, b"must survive").expect("witness must be writable");

    let root = scratch.outside("root");
    fs::create_dir_all(root.join(".pam/flows")).expect("root must be creatable");
    let store = Store::open(root.join("state.sqlite3")).expect("store must open");
    store.shutdown().await.expect("store must shut down");
    #[cfg(unix)]
    std::os::unix::fs::symlink(scratch.root.as_path(), root.join("escape"))
        .expect("symlink must be creatable");

    let paths = ResetPaths::for_state_path(&root.join("state.sqlite3")).expect("root must resolve");
    let context = ResetContext::new(
        paths,
        CredentialStore::Injected(Arc::new(MemorySecretBackend::default())),
    );

    let runtime = scratch.outside("free-runtime");
    fs::create_dir_all(&runtime).expect("runtime directory must be creatable");
    let endpoint = LocalEndpoint::ipc(runtime);

    let receipt = run_factory_reset(
        &context,
        &FactoryResetOptions::default(),
        &caller("cli"),
        &endpoint,
    )
    .await
    .expect("factory reset must succeed");

    assert!(
        outside.exists(),
        "a symlink out of the data root must be unlinked, never followed"
    );
    assert!(
        !root.join("escape").exists(),
        "the link itself must go, but only the link"
    );
    let _ = fs::remove_file(&receipt.path);
}

#[tokio::test]
async fn the_factory_audit_event_is_written_and_read_back_before_the_wipe() {
    let scratch = Scratch::new("factory-audit");
    let (store, context, _backend) = seeded(&scratch).await;

    let forecast = preview_tier_totals(&store, &context).await;
    let event_id = append_factory_audit(&store, &caller("cli"), &forecast)
        .await
        .expect("the reset must be able to record itself");
    confirm_audit_recorded(&store, &event_id)
        .await
        .expect("the event it just wrote must be readable back");

    let recorded = store
        .recent_audit_events(16)
        .await
        .expect("the ledger must be readable")
        .events
        .into_iter()
        .find(|event| event.event_id == event_id)
        .expect("the factory reset must appear in the ledger before the wipe");
    assert_eq!(recorded.action, "reset.factory");
    assert_eq!(recorded.decision, "allow");
    assert_eq!(recorded.outcome, "changed");
    assert!(recorded.redacted_detail.contains("scope=factory"));

    // A reset that cannot prove it recorded itself must never proceed.
    let error = confirm_audit_recorded(&store, "reset-factory-never-written")
        .await
        .expect_err("an unrecorded reset must refuse");
    assert!(matches!(error, ResetError::Store(_)));
    store.shutdown().await.expect("store must shut down");
}

#[tokio::test]
async fn a_factory_reset_that_cannot_open_the_ledger_changes_nothing() {
    let scratch = Scratch::new("factory-no-ledger");
    scratch.write(".pam/flows/release-readiness.toml", b"name = \"release\"\n");
    // A directory where the state database belongs: the ledger cannot open,
    // so the reset must refuse rather than wipe an unrecordable reset.
    fs::create_dir_all(scratch.path("state.sqlite3")).expect("blocker must be creatable");

    let context = ResetContext::new(
        ResetPaths::for_state_path(&scratch.path("state.sqlite3")).expect("root must resolve"),
        CredentialStore::Injected(Arc::new(MemorySecretBackend::default())),
    );
    let runtime = scratch.outside("free-runtime");
    fs::create_dir_all(&runtime).expect("runtime directory must be creatable");

    let error = run_factory_reset(
        &context,
        &FactoryResetOptions::default(),
        &caller("cli"),
        &LocalEndpoint::ipc(runtime),
    )
    .await
    .expect_err("an unopenable ledger must stop the reset");
    assert!(matches!(error, ResetError::Store(_)));
    assert!(
        scratch.path(".pam/flows/release-readiness.toml").exists(),
        "nothing may be removed when the reset cannot record itself"
    );
}
