#![forbid(unsafe_code)]

mod acquisition;
mod catalog;
mod error;
#[cfg(target_os = "macos")]
mod llama_cpp_macos;
mod memory;
mod model;
mod path;
mod runtime;

#[cfg(test)]
mod acquisition_test;
#[cfg(test)]
mod catalog_test;
#[cfg(all(test, target_os = "macos"))]
mod llama_cpp_macos_test;
#[cfg(test)]
mod memory_test;
#[cfg(test)]
mod model_test;
#[cfg(test)]
mod path_test;
#[cfg(test)]
mod runtime_test;

pub use acquisition::{
    DownloadRequest, DownloadResponse, DownloadTransport, ImportRequest, ModelFileReport,
    ReqwestDownloadTransport, TransferRequest, download_https, import_existing, inspect_model_file,
    revalidate_registered_model,
};
pub use catalog::{
    DanglingRegistration, ModelsDirectorySweep, OrphanWeights, WeightsRefusal,
    delete_registered_weights, health_label, sweep_models_directory, weights_deletion_allowed,
    weights_refusal_message,
};
pub use error::ModelError;
#[cfg(target_os = "macos")]
pub use llama_cpp_macos::{
    APPLICATION_RESERVE_BYTES, MacosLlamaCppRuntime, host_model_ceiling_bytes,
    host_projection_contingency_bytes, required_os_reserve,
};
pub use memory::{
    HostMemoryBudget, MemoryEstimate, MemoryEstimateError, MemoryFit, RuntimeMemoryProjection,
    UnifiedWorkingSetLimit, estimate_memory,
};
pub use model::{
    CALIBRATED_ARTIFACTS, CalibratedArtifact, GgufMetadata, LicenseConsent, LicenseSnapshot,
    ModelDescriptor, ModelKey, ModelSource, RegisteredModel, is_calibrated_artifact,
};
pub use path::{
    default_model_path, default_models_dir, effective_models_dir, model_path_under,
    validate_absolute_unicode_path, validate_model_filename,
};
pub use runtime::{
    ArtifactCalibration, CancellationSignal, CancellationToken, ModelRuntime, RuntimeError,
    RuntimeFinishReason, RuntimeFlashAttention, RuntimeGpuOffload, RuntimeHostAdmission,
    RuntimeHostSnapshot, RuntimeKvCachePrecision, RuntimeMemoryPressure, RuntimeMessage,
    RuntimeMessageRole, RuntimeProfile, RuntimeRequest, RuntimeResponse, RuntimeSampling,
    RuntimeSwapTrend, RuntimeUsage,
};
