//! The daemon's live model slot: one model at a time, changeable while the
//! daemon keeps serving.
//!
//! Before `model.load` existed, the model surface was a value cloned once per
//! request at startup and never written again. Loading and unloading make it
//! mutable, and mutability is where a runtime this large gets dangerous, so
//! the whole design is two rules:
//!
//! 1. **Readers take a snapshot, never a borrow.** A request clones the
//!    [`LoadedModelService`] out from under a short lock and then talks to
//!    that exact handle for its whole life. The handle is a channel to the
//!    worker task that owns the runtime, so an inference admitted a moment
//!    before a swap keeps talking to the model it was admitted against — it
//!    can never find a different model behind the same handle.
//! 2. **One transition at a time, old before new.** Every load and unload
//!    takes [`ModelCell::begin_transition`], and a swap drains and drops the
//!    outgoing worker completely before the incoming runtime is even built.
//!    The host therefore never carries two mapped artifacts at once, and the
//!    memory of the model that left is back before the model arriving asks
//!    for any.
//!
//! The cost of rule 2 is that a swap cancels an inference that is running at
//! the moment it starts. That is a clean, named ending —
//! [`pam_model::RuntimeError::Cancelled`] arrives at the caller as a
//! cancellation failure — not a half-swapped answer.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use pam_model::ModelKey;
use pam_protocol::{ModelTransition, ModelTransitionPhase};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

use crate::model_service::{ModelService, ModelWorker};

/// One model mapped into this daemon, and the single-flight service in front
/// of it.
#[derive(Clone)]
pub(crate) struct LoadedModelService {
    pub(crate) key: ModelKey,
    pub(crate) size_bytes: u64,
    pub(crate) service: ModelService,
}

/// A read-only snapshot of the model surface at one instant.
///
/// This is what `model.status` reports and what an inference resolves its
/// service from. It is a value, not a view: holding one never holds the cell's
/// lock, so a slow request can never stall a load, an unload, or another
/// status read.
#[derive(Clone, Default)]
pub(crate) struct ModelSurface {
    pub(crate) loaded: Option<LoadedModelService>,
    pub(crate) load_failure: Option<String>,
    pub(crate) transition: Option<ModelTransition>,
}

/// The outgoing model a transition took out of the cell, still to be drained.
///
/// Held by value on purpose: whoever takes it owes the runtime a
/// [`Retired::drain`], and the type makes forgetting that visible at the call
/// site rather than leaving a mapped model orphaned in the cell.
pub(crate) struct Retired {
    loaded: LoadedModelService,
    worker: Option<ModelWorker>,
}

impl Retired {
    /// The model that is on its way out, for the acknowledgement and audit.
    pub(crate) const fn key(&self) -> &ModelKey {
        &self.loaded.key
    }

    pub(crate) const fn size_bytes(&self) -> u64 {
        self.loaded.size_bytes
    }

    /// Cancels whatever this model was doing, waits for its worker task to
    /// end, and drops the runtime.
    ///
    /// Returning from here is the guarantee the swap rests on: the worker
    /// task owned the only [`std::sync::Arc`] to the runtime and awaited its
    /// own blocking generate call, so a joined worker is a dropped model and
    /// freed memory — not a hopeful `drop` racing the next load.
    pub(crate) async fn drain(self) {
        if let Some(worker) = self.worker {
            worker.shutdown().await;
        }
        drop(self.loaded);
    }
}

/// Exclusive permission to change the loaded model.
///
/// A live guard is the only thing that lets a load or an unload write the
/// cell, so two callers can never interleave halves of a swap. Dropping it
/// releases the transition for the next caller.
pub(crate) struct TransitionGuard {
    _guard: OwnedMutexGuard<()>,
}

struct CellState {
    loaded: Option<LoadedModelService>,
    worker: Option<ModelWorker>,
    load_failure: Option<String>,
    transition: Option<ModelTransition>,
}

/// The shared, mutable model slot every request handler reads and the load and
/// unload handlers write.
#[derive(Clone)]
pub(crate) struct ModelCell {
    state: Arc<Mutex<CellState>>,
    transition: Arc<AsyncMutex<()>>,
}

impl ModelCell {
    /// Seats the surface a daemon started with, including a startup load that
    /// failed: a degraded daemon reports its reason until something replaces
    /// it.
    pub(crate) fn new(surface: ModelSurface, worker: Option<ModelWorker>) -> Self {
        Self {
            state: Arc::new(Mutex::new(CellState {
                loaded: surface.loaded,
                worker,
                load_failure: surface.load_failure,
                transition: None,
            })),
            transition: Arc::new(AsyncMutex::new(())),
        }
    }

    fn lock(&self) -> MutexGuard<'_, CellState> {
        // A panic while the cell is locked would poison it, and every handler
        // reads it: recovering the guard keeps the daemon answering rather
        // than turning one poisoned read into total model unavailability.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The current surface, copied out so no reader holds the lock.
    pub(crate) fn surface(&self) -> ModelSurface {
        let state = self.lock();
        ModelSurface {
            loaded: state.loaded.clone(),
            load_failure: state.load_failure.clone(),
            transition: state.transition.clone(),
        }
    }

    /// The loaded model's identity without cloning the service handle.
    pub(crate) fn loaded_key(&self) -> Option<ModelKey> {
        self.lock().loaded.as_ref().map(|loaded| loaded.key.clone())
    }

    /// Claims the right to change the loaded model, or reports that another
    /// load or unload already holds it.
    ///
    /// Deliberately non-blocking: a caller that queued behind a multi-minute
    /// load would sit on the daemon's request handler with nothing to say.
    /// Refusing immediately gives the operator a sentence instead.
    pub(crate) fn begin_transition(&self) -> Option<TransitionGuard> {
        Arc::clone(&self.transition)
            .try_lock_owned()
            .ok()
            .map(|guard| TransitionGuard { _guard: guard })
    }

    /// Takes the loaded model out of the cell and announces the unload.
    ///
    /// The surface reports no loaded model from this moment on, so an
    /// inference that arrives during the drain answers the same not-loaded
    /// failure it answers when nothing was ever loaded. The returned
    /// [`Retired`] still owns the runtime until it is drained.
    pub(crate) fn begin_unload(&self, _guard: &TransitionGuard) -> Option<Retired> {
        let mut state = self.lock();
        let loaded = state.loaded.take()?;
        let worker = state.worker.take();
        state.transition = Some(ModelTransition {
            phase: ModelTransitionPhase::Unloading,
            model: loaded.key.id(),
        });
        Some(Retired { loaded, worker })
    }

    /// Settles the surface after an unload finishes draining.
    ///
    /// The stale load failure goes with it: whatever a previous load could not
    /// do is no longer the explanation for an empty slot the operator emptied
    /// themselves.
    pub(crate) fn finish_unload(&self, _guard: &TransitionGuard) {
        let mut state = self.lock();
        state.transition = None;
        state.load_failure = None;
    }

    /// Announces a load before the runtime work begins, so `model.status`
    /// reports `loading` for the minutes a multi-gigabyte artifact takes.
    pub(crate) fn begin_load(&self, _guard: &TransitionGuard, key: &ModelKey) {
        let mut state = self.lock();
        state.load_failure = None;
        state.transition = Some(ModelTransition {
            phase: ModelTransitionPhase::Loading,
            model: key.id(),
        });
    }

    /// Seats a model that came up, and ends the transition.
    pub(crate) fn finish_load(
        &self,
        _guard: &TransitionGuard,
        loaded: LoadedModelService,
        worker: Option<ModelWorker>,
    ) {
        let mut state = self.lock();
        state.loaded = Some(loaded);
        state.worker = worker;
        state.load_failure = None;
        state.transition = None;
    }

    /// Records a load that failed and ends the transition, leaving the daemon
    /// serving without a model.
    ///
    /// The same shape a failed startup load leaves behind: no loaded model,
    /// and a reason that survives on the surface for as long as this daemon
    /// runs rather than only in a log line nobody reads back.
    pub(crate) fn fail_load(&self, _guard: &TransitionGuard, reason: String) {
        let mut state = self.lock();
        state.loaded = None;
        state.worker = None;
        state.load_failure = Some(reason);
        state.transition = None;
    }

    /// Drains the loaded model at daemon shutdown.
    pub(crate) async fn shutdown(&self) {
        let worker = self.lock().worker.take();
        if let Some(worker) = worker {
            worker.shutdown().await;
        }
    }
}
