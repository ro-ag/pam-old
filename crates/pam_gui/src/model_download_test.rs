use std::{
    fs,
    future::Future,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use pam_model::{DownloadResponse, DownloadTransport, ModelError, TransferRequest};
use tokio::sync::Notify;
use uuid::Uuid;

#[cfg(target_os = "macos")]
use crate::model_download::host_memory_total_bytes;
use crate::model_download::{
    CountingResponse, ModelDownloadManager, ModelDownloadSnapshot, ModelDownloadStatusKind,
};
use crate::model_presets::ModelPreset;

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
    assert_eq!(snapshot.preset_id, None);
    assert_eq!(snapshot.received_bytes, 0);
    assert_eq!(snapshot.total_bytes, 0);
    assert!(snapshot.failure.is_none());
}

#[tokio::test]
async fn a_second_start_is_rejected_while_one_download_is_running() {
    let directory = TestDirectory::new("single-flight");
    let manager = ModelDownloadManager::new();
    let preset = tiny_preset();
    let holding = HoldingTransport::new();

    let started = Arc::clone(&manager).start_with(preset, directory.0.clone(), {
        let holding = holding.clone();
        move |_received| Ok(holding)
    });
    assert!(started.is_ok());

    holding.entered.notified().await;

    let rejected = Arc::clone(&manager).start_with(preset, directory.0.clone(), |_received| {
        Ok(HoldingTransport::new())
    });
    assert!(rejected.is_err());
    let snapshot_while_running = manager.snapshot();
    assert_eq!(
        snapshot_while_running.status,
        ModelDownloadStatusKind::Running
    );
    assert_eq!(snapshot_while_running.preset_id.as_deref(), Some(preset.id));

    holding.release.notify_one();
    let snapshot = wait_for_terminal(&manager).await;
    assert_eq!(snapshot.status, ModelDownloadStatusKind::Failed);
    assert_eq!(snapshot.preset_id.as_deref(), Some(preset.id));
    assert!(snapshot.failure.is_some());
}

#[tokio::test]
async fn a_new_download_can_start_once_the_previous_one_finished() {
    let directory = TestDirectory::new("restart-after-finish");
    let manager = ModelDownloadManager::new();
    let preset = tiny_preset();
    let holding = HoldingTransport::new();

    Arc::clone(&manager)
        .start_with(preset, directory.0.clone(), {
            let holding = holding.clone();
            move |_received| Ok(holding)
        })
        .unwrap();
    holding.entered.notified().await;
    holding.release.notify_one();
    wait_for_terminal(&manager).await;

    let restarted = Arc::clone(&manager).start_with(preset, directory.0.clone(), |_received| {
        Ok(HoldingTransport::new())
    });
    assert!(restarted.is_ok());
}

#[cfg(target_os = "macos")]
#[test]
fn host_memory_probe_reports_a_positive_total_on_macos() {
    assert!(host_memory_total_bytes().unwrap() > 0);
}
