use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use pam_model::{
    CancellationSignal, CancellationToken, ModelKey, RuntimeError, RuntimeFinishReason,
    RuntimeMessage, RuntimeMessageRole, RuntimeRequest, RuntimeResponse, RuntimeUsage,
};
use pam_protocol::ModelTransitionPhase;

use crate::{
    model_cell::{LoadedModelService, ModelCell, ModelSurface},
    model_service::{ModelGenerator, ModelService, ModelServiceError, ModelWorker},
};

/// A generator that answers with its own name, so a response proves which
/// model produced it — the whole point of the swap tests below.
struct NamedGenerator {
    name: &'static str,
    active: AtomicBool,
    calls: AtomicUsize,
    release: AtomicBool,
}

impl NamedGenerator {
    fn new(name: &'static str, release: bool) -> Self {
        Self {
            name,
            active: AtomicBool::new(false),
            calls: AtomicUsize::new(0),
            release: AtomicBool::new(release),
        }
    }
}

impl ModelGenerator for NamedGenerator {
    fn generate(
        &self,
        _request: RuntimeRequest,
        cancellation: CancellationToken,
    ) -> Result<RuntimeResponse, RuntimeError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.active.store(true, Ordering::SeqCst);
        while !self.release.load(Ordering::SeqCst) {
            if cancellation.is_cancelled() {
                self.active.store(false, Ordering::SeqCst);
                return Err(RuntimeError::Cancelled);
            }
            thread::sleep(Duration::from_millis(1));
        }
        self.active.store(false, Ordering::SeqCst);
        Ok(RuntimeResponse {
            text: self.name.to_owned(),
            finish_reason: RuntimeFinishReason::Stop,
            usage: RuntimeUsage {
                input_tokens: 1,
                sampled_output_tokens: 1,
                emitted_output_tokens: 1,
            },
        })
    }
}

fn key(name: &str) -> ModelKey {
    ModelKey::new("qwen", name).unwrap()
}

fn request() -> RuntimeRequest {
    RuntimeRequest::new(
        vec![RuntimeMessage::new(RuntimeMessageRole::User, "hello").unwrap()],
        16,
    )
    .unwrap()
}

/// Stands in for the real runtime load: one model, one worker, one service.
fn mount(
    name: &'static str,
    model: &str,
    size_bytes: u64,
    release: bool,
) -> (Arc<NamedGenerator>, LoadedModelService, ModelWorker) {
    let generator = Arc::new(NamedGenerator::new(name, release));
    let (service, worker) = ModelService::start_generator(Arc::clone(&generator));
    (
        generator,
        LoadedModelService {
            key: key(model),
            size_bytes,
            service,
        },
        worker,
    )
}

async fn wait_until_active(generator: &NamedGenerator) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !generator.active.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn an_empty_cell_loads_unloads_and_loads_a_different_model() {
    let cell = ModelCell::new(ModelSurface::default(), None);
    assert!(cell.surface().loaded.is_none());
    assert!(cell.surface().transition.is_none());
    assert!(cell.loaded_key().is_none());

    // Load into a daemon holding nothing.
    let guard = cell.begin_transition().expect("an idle cell is free");
    let (first_generator, first, first_worker) = mount("first", "first", 10, true);
    cell.begin_load(&guard, &key("first"));
    let loading = cell.surface().transition.expect("a load reports itself");
    assert_eq!(loading.phase, ModelTransitionPhase::Loading);
    assert_eq!(loading.model, "qwen/first");
    cell.finish_load(&guard, first, Some(first_worker));
    drop(guard);

    let surface = cell.surface();
    assert!(surface.transition.is_none());
    let loaded = surface.loaded.expect("the model is seated");
    assert_eq!(loaded.key.id(), "qwen/first");
    assert_eq!(loaded.size_bytes, 10);
    assert_eq!(
        loaded
            .service
            .infer(request(), Instant::now() + Duration::from_secs(2))
            .await
            .unwrap()
            .text,
        "first"
    );

    // Unload, and the slot is empty again.
    let guard = cell.begin_transition().unwrap();
    let retired = cell.begin_unload(&guard).expect("a loaded cell retires");
    assert_eq!(retired.key().id(), "qwen/first");
    assert_eq!(retired.size_bytes(), 10);
    let unloading = cell
        .surface()
        .transition
        .expect("an unload reports itself while it drains");
    assert_eq!(unloading.phase, ModelTransitionPhase::Unloading);
    assert_eq!(unloading.model, "qwen/first");
    // The surface already reports nothing loaded, so inference answers the
    // one not-loaded failure rather than a second "unloading" state.
    assert!(cell.surface().loaded.is_none());
    retired.drain().await;
    cell.finish_unload(&guard);
    drop(guard);
    assert!(cell.surface().loaded.is_none());
    assert!(cell.surface().transition.is_none());
    assert_eq!(first_generator.calls.load(Ordering::SeqCst), 1);

    // Load a different model into the same daemon.
    let guard = cell.begin_transition().unwrap();
    let (_, second, second_worker) = mount("second", "second", 20, true);
    cell.begin_load(&guard, &key("second"));
    cell.finish_load(&guard, second, Some(second_worker));
    drop(guard);
    let loaded = cell.surface().loaded.expect("the second model is seated");
    assert_eq!(loaded.key.id(), "qwen/second");
    assert_eq!(
        loaded
            .service
            .infer(request(), Instant::now() + Duration::from_secs(2))
            .await
            .unwrap()
            .text,
        "second"
    );
    cell.shutdown().await;
}

#[tokio::test]
async fn a_swap_drains_the_old_model_before_the_new_one_is_admitted() {
    let (first_generator, first, first_worker) = mount("first", "first", 10, false);
    let cell = ModelCell::new(
        ModelSurface {
            loaded: Some(first),
            load_failure: None,
            transition: None,
        },
        Some(first_worker),
    );

    // An inference is running against the model that is about to be replaced.
    // It resolved its service handle from the cell before the swap started,
    // exactly as a request handler does.
    let in_flight = cell.surface().loaded.unwrap().service;
    let inference = tokio::spawn(async move {
        in_flight
            .infer(request(), Instant::now() + Duration::from_secs(5))
            .await
    });
    wait_until_active(&first_generator).await;

    let guard = cell.begin_transition().unwrap();
    let retired = cell
        .begin_unload(&guard)
        .expect("the swap retires the old model");
    retired.drain().await;
    // Draining returned, so the outgoing worker has joined and its runtime is
    // dropped: nothing of the old model is alive when the new one arrives.
    assert!(!first_generator.active.load(Ordering::SeqCst));

    let (second_generator, second, second_worker) = mount("second", "second", 20, true);
    cell.begin_load(&guard, &key("second"));
    cell.finish_load(&guard, second, Some(second_worker));
    drop(guard);

    // The in-flight request ends as a named cancellation against the model it
    // was admitted to. It never sees the new model, and the new model never
    // sees it.
    let observed = inference.await.unwrap();
    assert!(
        matches!(
            observed,
            Err(ModelServiceError::Runtime(RuntimeError::Cancelled))
        ),
        "an in-flight inference must end cleanly, not half-swapped: {observed:?}"
    );
    assert_eq!(second_generator.calls.load(Ordering::SeqCst), 0);

    // And the seated model is the new one, answering for itself.
    assert_eq!(
        cell.surface()
            .loaded
            .unwrap()
            .service
            .infer(request(), Instant::now() + Duration::from_secs(2))
            .await
            .unwrap()
            .text,
        "second"
    );
    cell.shutdown().await;
}

#[tokio::test]
async fn only_one_transition_holds_the_model_slot_at_a_time() {
    let cell = ModelCell::new(ModelSurface::default(), None);
    let guard = cell.begin_transition().expect("an idle cell is free");
    assert!(
        cell.begin_transition().is_none(),
        "a second load or unload must be refused, not silently queued"
    );
    drop(guard);
    assert!(cell.begin_transition().is_some());
}

#[tokio::test]
async fn a_failed_load_leaves_the_daemon_serving_with_the_reason_on_the_surface() {
    let (_, first, first_worker) = mount("first", "first", 10, true);
    let cell = ModelCell::new(
        ModelSurface {
            loaded: Some(first),
            load_failure: None,
            transition: None,
        },
        Some(first_worker),
    );

    let guard = cell.begin_transition().unwrap();
    let retired = cell.begin_unload(&guard).unwrap();
    retired.drain().await;
    cell.begin_load(&guard, &key("second"));
    cell.fail_load(&guard, "model load failed: weights drifted".to_owned());
    drop(guard);

    let surface = cell.surface();
    assert!(surface.loaded.is_none());
    assert!(surface.transition.is_none());
    assert_eq!(
        surface.load_failure.as_deref(),
        Some("model load failed: weights drifted")
    );

    // A later unload of nothing is refused, and a successful one clears the
    // stale reason rather than leaving it explaining an empty slot forever.
    let guard = cell.begin_transition().unwrap();
    assert!(cell.begin_unload(&guard).is_none());
    cell.finish_unload(&guard);
    drop(guard);
    assert!(cell.surface().load_failure.is_none());
}
