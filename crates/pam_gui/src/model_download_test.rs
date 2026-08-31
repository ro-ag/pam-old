// The fake host resolvers below stand in for system DNS, so they must match
// `HostResolver`'s fn-pointer signature — the `Result` is load-bearing even in
// the cases that cannot fail.
#![allow(clippy::unnecessary_wraps)]

use std::{
    fs,
    future::Future,
    net::{IpAddr, Ipv4Addr},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use pam_core::ContentDigest;
use pam_model::{DownloadResponse, DownloadTransport, ModelError, TransferRequest};
use sha2::{Digest as _, Sha256};
use tokio::sync::Notify;
use uuid::Uuid;

#[cfg(target_os = "macos")]
use crate::model_download::host_memory_total_bytes;
use crate::model_download::{
    CountingResponse, CountingTransport, ModelAcquisition, ModelDownloadKind, ModelDownloadManager,
    ModelDownloadSnapshot, ModelDownloadStatusKind,
};
use crate::model_presets::ModelPreset;
use crate::model_url_download::{ModelUrlDownloadParams, acquisition_from_url};

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("pam-gui-download-{name}-{}", Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The acquisition the download manager actually runs, built from the same
/// preset the curated path builds one from.
fn tiny_acquisition() -> ModelAcquisition {
    ModelAcquisition::from_preset(&tiny_preset()).unwrap()
}

fn tiny_preset() -> ModelPreset {
    ModelPreset {
        id: "test-preset",
        label: "Test Preset",
        model: "acme/test-model",
        file_name: "test-model.gguf",
        url: "https://huggingface.co/acme/test-model/resolve/main/test-model.gguf",
        expected_size_bytes: 4096,
        sha256: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        license_id: "Test-License",
        license_url: "https://example.test/license",
        license_notice_text: "test-model.gguf is distributed under the Test-License license.",
        params_label: "0B",
        quant_label: "Q4_K_M",
    }
}

struct FakeResponse {
    chunks: Vec<Vec<u8>>,
}

impl DownloadResponse for FakeResponse {
    fn status(&self) -> u16 {
        200
    }

    fn content_length(&self) -> Option<&str> {
        None
    }

    fn content_range(&self) -> Option<&str> {
        None
    }

    fn content_encoding(&self) -> Option<&str> {
        None
    }

    fn etag(&self) -> Option<&str> {
        None
    }

    fn location(&self) -> Option<&str> {
        None
    }

    fn next_chunk(&mut self) -> impl Future<Output = Result<Option<Vec<u8>>, ModelError>> + Send {
        let chunk = if self.chunks.is_empty() {
            None
        } else {
            Some(self.chunks.remove(0))
        };
        std::future::ready(Ok(chunk))
    }
}

#[tokio::test]
async fn counting_response_adds_each_chunk_length_and_stops_at_none() {
    let received = Arc::new(AtomicU64::new(0));
    let mut response = CountingResponse::new(
        FakeResponse {
            chunks: vec![vec![0; 10], vec![0; 5], vec![0; 7]],
        },
        Arc::clone(&received),
    );

    assert_eq!(response.next_chunk().await.unwrap().unwrap().len(), 10);
    assert_eq!(received.load(Ordering::Relaxed), 10);
    assert_eq!(response.next_chunk().await.unwrap().unwrap().len(), 5);
    assert_eq!(received.load(Ordering::Relaxed), 15);
    assert_eq!(response.next_chunk().await.unwrap().unwrap().len(), 7);
    assert_eq!(received.load(Ordering::Relaxed), 22);
    assert!(response.next_chunk().await.unwrap().is_none());
    assert_eq!(received.load(Ordering::Relaxed), 22);
}

struct NeverResponse;

impl DownloadResponse for NeverResponse {
    fn status(&self) -> u16 {
        unreachable!("HoldingTransport::send never returns Ok")
    }

    fn content_length(&self) -> Option<&str> {
        unreachable!()
    }

    fn content_range(&self) -> Option<&str> {
        unreachable!()
    }

    fn content_encoding(&self) -> Option<&str> {
        unreachable!()
    }

    fn etag(&self) -> Option<&str> {
        unreachable!()
    }

    fn location(&self) -> Option<&str> {
        unreachable!()
    }

    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, ModelError> {
        unreachable!()
    }
}

/// A transport whose `send` blocks until released, then fails. Lets a test
/// observe a download mid-flight without any real network I/O.
#[derive(Clone)]
struct HoldingTransport {
    entered: Arc<Notify>,
    release: Arc<Notify>,
}

impl HoldingTransport {
    fn new() -> Self {
        Self {
            entered: Arc::new(Notify::new()),
            release: Arc::new(Notify::new()),
        }
    }
}

impl DownloadTransport for HoldingTransport {
    type Response = NeverResponse;

    async fn send(&self, _request: TransferRequest) -> Result<Self::Response, ModelError> {
        self.entered.notify_one();
        self.release.notified().await;
        Err(ModelError::Network)
    }
}

async fn wait_for_terminal(manager: &Arc<ModelDownloadManager>) -> ModelDownloadSnapshot {
    for _ in 0..200 {
        let snapshot = manager.snapshot();
        if snapshot.status != ModelDownloadStatusKind::Running {
            return snapshot;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("download manager never reached a terminal state");
}

#[tokio::test]
async fn a_fresh_manager_is_idle() {
    let manager = ModelDownloadManager::new();
    let snapshot = manager.snapshot();
    assert_eq!(snapshot.status, ModelDownloadStatusKind::Idle);
    assert_eq!(snapshot.download_id, None);
    assert_eq!(snapshot.download_kind, None);
    assert_eq!(snapshot.received_bytes, 0);
    assert_eq!(snapshot.total_bytes, 0);
    assert!(snapshot.failure.is_none());
}

#[tokio::test]
async fn a_second_start_is_rejected_while_one_download_is_running() {
    let directory = TestDirectory::new("single-flight");
    let manager = ModelDownloadManager::new();
    let acquisition = tiny_acquisition();
    let holding = HoldingTransport::new();

    let started = Arc::clone(&manager).start_with(acquisition.clone(), directory.0.clone(), {
        let holding = holding.clone();
        move |_received, _cancel| Ok(holding)
    });
    assert!(started.is_ok());

    holding.entered.notified().await;

    let rejected = Arc::clone(&manager).start_with(
        acquisition.clone(),
        directory.0.clone(),
        |_received, _cancel| Ok(HoldingTransport::new()),
    );
    assert!(rejected.is_err());
    let snapshot_while_running = manager.snapshot();
    assert_eq!(
        snapshot_while_running.status,
        ModelDownloadStatusKind::Running
    );
    assert_eq!(
        snapshot_while_running.download_id.as_deref(),
        Some(acquisition.id.as_str())
    );
    assert_eq!(
        snapshot_while_running.download_kind,
        Some(ModelDownloadKind::Preset)
    );

    holding.release.notify_one();
    let snapshot = wait_for_terminal(&manager).await;
    assert_eq!(snapshot.status, ModelDownloadStatusKind::Failed);
    assert_eq!(
        snapshot.download_id.as_deref(),
        Some(acquisition.id.as_str())
    );
    assert!(snapshot.failure.is_some());
}

#[tokio::test]
async fn a_new_download_can_start_once_the_previous_one_finished() {
    let directory = TestDirectory::new("restart-after-finish");
    let manager = ModelDownloadManager::new();
    let acquisition = tiny_acquisition();
    let holding = HoldingTransport::new();

    Arc::clone(&manager)
        .start_with(acquisition.clone(), directory.0.clone(), {
            let holding = holding.clone();
            move |_received, _cancel| Ok(holding)
        })
        .unwrap();
    holding.entered.notified().await;
    holding.release.notify_one();
    wait_for_terminal(&manager).await;

    let restarted =
        Arc::clone(&manager).start_with(acquisition, directory.0.clone(), |_received, _cancel| {
            Ok(HoldingTransport::new())
        });
    assert!(restarted.is_ok());
}

/// A response that drops the connection partway through, like a real
/// interrupted transfer: it yields its chunks and then ends (`Ok(None)`)
/// without ever reaching the descriptor's expected size. Carries a strong
/// `ETag` so `pam_model` treats the partial bytes it leaves on disk as safe
/// to resume rather than discarding them.
struct InterruptedResponse {
    etag: &'static str,
    chunks: Vec<Vec<u8>>,
}

impl DownloadResponse for InterruptedResponse {
    fn status(&self) -> u16 {
        200
    }

    fn content_length(&self) -> Option<&str> {
        None
    }

    fn content_range(&self) -> Option<&str> {
        None
    }

    fn content_encoding(&self) -> Option<&str> {
        None
    }

    fn etag(&self) -> Option<&str> {
        Some(self.etag)
    }

    fn location(&self) -> Option<&str> {
        None
    }

    fn next_chunk(&mut self) -> impl Future<Output = Result<Option<Vec<u8>>, ModelError>> + Send {
        let chunk = if self.chunks.is_empty() {
            None
        } else {
            Some(self.chunks.remove(0))
        };
        std::future::ready(Ok(chunk))
    }
}

struct InterruptedTransport(std::sync::Mutex<Option<InterruptedResponse>>);

impl DownloadTransport for InterruptedTransport {
    type Response = InterruptedResponse;

    async fn send(&self, _request: TransferRequest) -> Result<Self::Response, ModelError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .take()
            .expect("one response per test"))
    }
}

#[tokio::test]
async fn a_resumed_download_seeds_the_counter_from_the_existing_partial_offset() {
    let directory = TestDirectory::new("resume-seed");
    let acquisition = tiny_acquisition();

    // Phase 1: a real (fake-transport-backed) download that drops partway
    // through, leaving a genuine partial file and matching checkpoint on
    // disk — exactly what a resumed download resumes from.
    let interrupted = ModelDownloadManager::new();
    Arc::clone(&interrupted)
        .start_with(
            acquisition.clone(),
            directory.0.clone(),
            |_received, _cancel| {
                Ok(InterruptedTransport(std::sync::Mutex::new(Some(
                    InterruptedResponse {
                        etag: "\"stable\"",
                        chunks: vec![vec![0; 1024]],
                    },
                ))))
            },
        )
        .unwrap();
    let interrupted_snapshot = wait_for_terminal(&interrupted).await;
    assert_eq!(interrupted_snapshot.status, ModelDownloadStatusKind::Failed);

    // Phase 2: resuming the same destination must start the counter at the
    // offset already on disk, not zero.
    let resumed = ModelDownloadManager::new();
    let holding = HoldingTransport::new();
    Arc::clone(&resumed)
        .start_with(acquisition, directory.0.clone(), {
            let holding = holding.clone();
            move |_received, _cancel| Ok(holding)
        })
        .unwrap();

    holding.entered.notified().await;
    assert_eq!(resumed.snapshot().received_bytes, 1024);

    holding.release.notify_one();
    wait_for_terminal(&resumed).await;
}

/// A response that never runs out of chunks: only cancellation (or an
/// oversize refusal) can end this transfer, which is exactly what the
/// cancel path must handle.
struct EndlessResponse;

impl DownloadResponse for EndlessResponse {
    fn status(&self) -> u16 {
        200
    }

    fn content_length(&self) -> Option<&str> {
        None
    }

    fn content_range(&self) -> Option<&str> {
        None
    }

    fn content_encoding(&self) -> Option<&str> {
        None
    }

    fn etag(&self) -> Option<&str> {
        Some("\"stable\"")
    }

    fn location(&self) -> Option<&str> {
        None
    }

    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, ModelError> {
        // Pace the chunks so the test body observes progress and cancels
        // long before the transfer could overshoot the preset size.
        tokio::time::sleep(Duration::from_millis(5)).await;
        Ok(Some(vec![0; 16]))
    }
}

struct EndlessTransport;

impl DownloadTransport for EndlessTransport {
    type Response = EndlessResponse;

    async fn send(&self, _request: TransferRequest) -> Result<Self::Response, ModelError> {
        Ok(EndlessResponse)
    }
}

#[tokio::test]
async fn cancel_stops_a_running_download_as_cancelled_not_failed() {
    let directory = TestDirectory::new("cancel-running");
    let manager = ModelDownloadManager::new();
    let acquisition = tiny_acquisition();

    Arc::clone(&manager)
        .start_with(
            acquisition.clone(),
            directory.0.clone(),
            |received, cancel| {
                Ok(CountingTransport {
                    inner: EndlessTransport,
                    received,
                    cancel,
                })
            },
        )
        .unwrap();

    // Wait until the transfer is demonstrably moving, then cancel it.
    for _ in 0..200 {
        if manager.snapshot().received_bytes > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    manager.cancel().unwrap();

    let snapshot = wait_for_terminal(&manager).await;
    assert_eq!(snapshot.status, ModelDownloadStatusKind::Cancelled);
    assert_eq!(
        snapshot.download_id.as_deref(),
        Some(acquisition.id.as_str())
    );
    assert!(snapshot.failure.is_none());

    // A cancelled manager accepts a fresh start.
    let restarted = Arc::clone(&manager).start_with(acquisition, directory.0.clone(), {
        let holding = HoldingTransport::new();
        move |_received, _cancel| Ok(holding)
    });
    assert!(restarted.is_ok());
}

#[tokio::test]
async fn cancel_without_a_running_download_is_refused() {
    let manager = ModelDownloadManager::new();
    let refused = manager.cancel().unwrap_err();
    assert_eq!(refused.detail, "No model download is running.");
    assert_eq!(manager.snapshot().status, ModelDownloadStatusKind::Idle);
}

#[cfg(target_os = "macos")]
#[test]
fn host_memory_probe_reports_a_positive_total_on_macos() {
    assert!(host_memory_total_bytes().unwrap() > 0);
}

/// A minimal, structurally valid one-tensor GGUF, so a download reaches
/// `pam_model`'s digest and size checks instead of being turned away as a
/// malformed file first.
fn tiny_gguf(payload_bytes: usize) -> Vec<u8> {
    let mut bytes = b"GGUF".to_vec();
    bytes.extend_from_slice(&3_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u64.to_le_bytes());
    bytes.extend_from_slice(&0_u64.to_le_bytes());
    bytes.extend_from_slice(&6_u64.to_le_bytes());
    bytes.extend_from_slice(b"weight");
    bytes.extend_from_slice(&1_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u64.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&0_u64.to_le_bytes());
    while !bytes.len().is_multiple_of(32) {
        bytes.push(0);
    }
    bytes.resize(bytes.len() + payload_bytes, 0);
    bytes
}

/// Stands in for system DNS: every host answers with one public address, so
/// the pasted-URL gate's address check passes and the test exercises the
/// download itself.
fn public_resolver(_host: &str) -> Result<Vec<IpAddr>, std::io::Error> {
    Ok(vec![IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))])
}

fn pasted_params(sha256: &str, expected_size_bytes: u64) -> ModelUrlDownloadParams {
    ModelUrlDownloadParams {
        model: "acme/pasted-model".to_owned(),
        url: "https://models.example/pasted-model.gguf".to_owned(),
        expected_size_bytes,
        sha256: sha256.to_owned(),
        license_id: "Test-License".to_owned(),
        license_url: "https://example.test/license".to_owned(),
        license_notice_text: "pasted-model.gguf is under the Test-License license.".to_owned(),
        accepted: true,
    }
}

fn pasted_acquisition(sha256: &str, expected_size_bytes: u64) -> ModelAcquisition {
    acquisition_from_url(&pasted_params(sha256, expected_size_bytes), public_resolver).unwrap()
}

/// A transport that answers with one body of caller-chosen bytes.
struct BodyTransport(std::sync::Mutex<Option<Vec<u8>>>);

impl BodyTransport {
    fn new(body: Vec<u8>) -> Self {
        Self(std::sync::Mutex::new(Some(body)))
    }
}

impl DownloadTransport for BodyTransport {
    type Response = FakeResponse;

    async fn send(&self, _request: TransferRequest) -> Result<Self::Response, ModelError> {
        Ok(FakeResponse {
            chunks: vec![
                self.0
                    .lock()
                    .unwrap()
                    .take()
                    .expect("one response per test"),
            ],
        })
    }
}

#[tokio::test]
async fn a_pasted_url_download_whose_bytes_miss_the_digest_registers_nothing() {
    let directory = TestDirectory::new("pasted-digest-mismatch");
    let manager = ModelDownloadManager::new();
    let bytes = tiny_gguf(64);
    // The digest the user pasted is not the digest of what the source sends.
    let acquisition = pasted_acquisition(
        "1111111111111111111111111111111111111111111111111111111111111111",
        u64::try_from(bytes.len()).unwrap(),
    );

    Arc::clone(&manager)
        .start_with(acquisition, directory.0.clone(), move |_r, _c| {
            Ok(BodyTransport::new(bytes))
        })
        .unwrap();

    let snapshot = wait_for_terminal(&manager).await;
    assert_eq!(snapshot.status, ModelDownloadStatusKind::Failed);
    assert_eq!(snapshot.download_kind, Some(ModelDownloadKind::Url));
    let failure = snapshot.failure.unwrap();
    assert_eq!(
        failure.detail,
        "model SHA-256 did not match the expected digest"
    );
    assert!(
        failure
            .recovery
            .unwrap()
            .contains("Nothing was registered and the partial file was discarded.")
    );
    // Neither the published file nor the partial survives a failed digest.
    assert!(!directory.0.join("acme").join("pasted-model.gguf").exists());
    assert!(
        !directory
            .0
            .join("acme")
            .join(".pasted-model.gguf.pam-model.part")
            .exists()
    );
}

#[tokio::test]
async fn a_pasted_url_download_that_overruns_its_declared_size_is_refused() {
    let directory = TestDirectory::new("pasted-size-mismatch");
    let manager = ModelDownloadManager::new();
    let bytes = tiny_gguf(64);
    let digest = ContentDigest::from_sha256(Sha256::digest(&bytes).into());
    let declared = u64::try_from(bytes.len()).unwrap() - 32;
    let acquisition =
        pasted_acquisition(digest.as_str().strip_prefix("sha256:").unwrap(), declared);

    Arc::clone(&manager)
        .start_with(acquisition, directory.0.clone(), move |_r, _c| {
            Ok(BodyTransport::new(bytes))
        })
        .unwrap();

    let snapshot = wait_for_terminal(&manager).await;
    assert_eq!(snapshot.status, ModelDownloadStatusKind::Failed);
    let failure = snapshot.failure.unwrap();
    assert!(
        failure.detail.starts_with("model size mismatch"),
        "unexpected detail: {}",
        failure.detail
    );
    assert!(!directory.0.join("acme").join("pasted-model.gguf").exists());
}

#[tokio::test]
async fn cancel_and_resume_work_the_same_way_for_a_pasted_url() {
    let directory = TestDirectory::new("pasted-cancel-resume");
    let manager = ModelDownloadManager::new();
    let acquisition = pasted_acquisition(
        "2222222222222222222222222222222222222222222222222222222222222222",
        4096,
    );

    Arc::clone(&manager)
        .start_with(
            acquisition.clone(),
            directory.0.clone(),
            |received, cancel| {
                Ok(CountingTransport {
                    inner: EndlessTransport,
                    received,
                    cancel,
                })
            },
        )
        .unwrap();

    for _ in 0..200 {
        if manager.snapshot().received_bytes > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    manager.cancel().unwrap();

    let cancelled = wait_for_terminal(&manager).await;
    assert_eq!(cancelled.status, ModelDownloadStatusKind::Cancelled);
    assert_eq!(cancelled.download_id.as_deref(), Some("acme/pasted-model"));
    assert_eq!(cancelled.download_kind, Some(ModelDownloadKind::Url));
    assert!(cancelled.failure.is_none());
    let kept = cancelled.received_bytes;
    assert!(kept > 0);

    // Restarting the same pasted URL resumes from the bytes already on disk
    // rather than beginning again at zero.
    let holding = HoldingTransport::new();
    Arc::clone(&manager)
        .start_with(acquisition, directory.0.clone(), {
            let holding = holding.clone();
            move |_received, _cancel| Ok(holding)
        })
        .unwrap();
    holding.entered.notified().await;
    assert_eq!(manager.snapshot().received_bytes, kept);
    holding.release.notify_one();
    wait_for_terminal(&manager).await;
}

#[test]
fn a_pasted_url_never_inherits_the_preset_redirect_allowlist() {
    let pasted = pasted_acquisition(
        "3333333333333333333333333333333333333333333333333333333333333333",
        4096,
    );
    // Empty here means `pam_model` allows only the pasted URL's own host,
    // which it appends itself — no cross-host hop for a source PAM did not
    // hand-check.
    assert!(pasted.allowed_redirect_hosts.is_empty());
    assert!(
        !ModelAcquisition::from_preset(&tiny_preset())
            .unwrap()
            .allowed_redirect_hosts
            .is_empty()
    );
}

#[test]
fn a_pasted_url_records_the_plain_canonical_source() {
    let pasted = pasted_acquisition(
        "4444444444444444444444444444444444444444444444444444444444444444",
        4096,
    );
    assert_eq!(pasted.url, "https://models.example/pasted-model.gguf");
    assert!(!pasted.url.contains('?'));
    assert!(!pasted.url.contains('#'));
    assert_eq!(pasted.descriptor.filename, "pasted-model.gguf");
    assert_eq!(pasted.id, "acme/pasted-model");
}
