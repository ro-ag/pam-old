use std::path::PathBuf;

use llama_cpp_4::{
    context::params::LlamaFlashAttnType,
    model::{LlamaChatMessage, params::LlamaModelParams},
    quantize::GgmlType,
};
use pam_core::ContentDigest;

use super::{
    ArtifactCalibration, GgufMetadata, LicenseSnapshot, ModelKey, ModelSource, RegisteredModel,
    RuntimeError, RuntimeFinishReason, RuntimeFlashAttention, RuntimeGpuOffload,
    RuntimeHostSnapshot, RuntimeKvCachePrecision, RuntimeMemoryPressure, RuntimeMemoryProjection,
    RuntimeMessage, RuntimeMessageRole, RuntimeRequest, RuntimeSampling, RuntimeSwapTrend,
};
use crate::CALIBRATED_ARTIFACTS;
use crate::llama_cpp_macos::{
    APPLICATION_RESERVE_BYTES, admit_bytes, artifact_calibration, bounded_template_size,
    build_chat_messages, calibrated_contingency, calibrated_runtime_profile,
    final_prefill_sample_slot, fixed_context_params, fixed_model_params, host_model_ceiling_bytes,
    host_projection_contingency_bytes, metal_and_host_memory_limits, prefill_chunk_ranges,
    projection_from_entries, required_os_reserve, test_calibrated_digest, test_calibrated_size,
    validate_host_admission,
};

const GIB: u64 = 1024 * 1024 * 1024;
/// PAM's documented minimum Mac; its ceiling is the safety floor no host
/// derivation may loosen.
const MIN_HOST_BYTES: u64 = 32 * GIB;
const OWNER_HOST_BYTES: u64 = 64 * GIB;
/// `qwen3next/qwen3-coder-next` weights: over the retired 27 GB constant.
const LARGE_ARTIFACT_BYTES: u64 = 39_234_725_888;

fn registered(digest: ContentDigest, size_bytes: u64) -> RegisteredModel {
    RegisteredModel {
        key: ModelKey::new("qwen", "qwen3-coder-30b-a3b-instruct-q4-k-s").unwrap(),
        path: PathBuf::from("/tmp/model.gguf"),
        digest,
        size_bytes,
        gguf: GgufMetadata {
            version: 3,
            tensor_count: 1,
            metadata_kv_count: 1,
            architecture: None,
            model_name: None,
            license: None,
        },
        license: LicenseSnapshot::new(
            "Apache-2.0",
            "https://example.test/license",
            ContentDigest::from_sha256([7; 32]),
        )
        .unwrap(),
        source: ModelSource::Local,
        registered_at_ms: 0,
    }
}

#[test]
fn exact_digest_and_size_are_calibrated_and_anything_else_is_not() {
    let ceiling = host_model_ceiling_bytes(MIN_HOST_BYTES);
    let digest = ContentDigest::parse(test_calibrated_digest()).unwrap();
    assert_eq!(
        artifact_calibration(&registered(digest.clone(), test_calibrated_size()), ceiling).unwrap(),
        ArtifactCalibration::Calibrated
    );
    // Same digest, one byte short, and a stranger digest at the exact size:
    // both leave the calibrated set but still fit this host.
    assert_eq!(
        artifact_calibration(&registered(digest, test_calibrated_size() - 1), ceiling).unwrap(),
        ArtifactCalibration::Uncalibrated
    );
    assert_eq!(
        artifact_calibration(
            &registered(ContentDigest::from_sha256([0; 32]), test_calibrated_size()),
            ceiling,
        )
        .unwrap(),
        ArtifactCalibration::Uncalibrated
    );
}

#[test]
fn calibrated_profile_accepts_every_calibrated_artifact() {
    let ceiling = host_model_ceiling_bytes(MIN_HOST_BYTES);
    for artifact in CALIBRATED_ARTIFACTS {
        let digest = ContentDigest::parse(format!("sha256:{}", artifact.digest)).unwrap();
        assert_eq!(
            artifact_calibration(&registered(digest, artifact.size_bytes), ceiling).unwrap(),
            ArtifactCalibration::Calibrated,
            "{} should be an accepted calibrated artifact",
            artifact.digest
        );
    }
}

/// PAM ships a calibrated artifact its own documented minimum Mac cannot run.
/// Calibration means "measured end to end", not "fits every supported host",
/// so the honest assertion is per-host: every calibrated artifact clears
/// admission on a host that can hold it, and the Q6\_K is refused on the
/// 32 GiB minimum with a numbered message.
#[test]
fn every_calibrated_artifact_passes_admission_on_a_host_that_can_hold_it() {
    let min_ceiling = host_model_ceiling_bytes(MIN_HOST_BYTES);
    let owner_ceiling = host_model_ceiling_bytes(OWNER_HOST_BYTES);
    for artifact in CALIBRATED_ARTIFACTS {
        let contingency = calibrated_contingency(artifact.size_bytes);
        let admitted = artifact.size_bytes.checked_add(contingency).unwrap();
        // The 64 GiB Mac every artifact was measured on holds all of them.
        assert!(
            admit_bytes(admitted, owner_ceiling).is_ok(),
            "{} should pass admission with its calibrated contingency on a 64 GiB Mac",
            artifact.digest
        );
        // On the 32 GiB minimum, exactly the Q6_K is over the ceiling. This
        // is pinned per artifact so a new catalog entry cannot quietly join
        // it.
        assert_eq!(
            admit_bytes(admitted, min_ceiling).is_ok(),
            artifact.size_bytes != LARGEST_CALIBRATED_BYTES,
            "{} changed its minimum-Mac admission verdict",
            artifact.digest
        );
    }

    let over_minimum = LARGEST_CALIBRATED_BYTES
        .checked_add(calibrated_contingency(LARGEST_CALIBRATED_BYTES))
        .unwrap();
    let error = admit_bytes(over_minimum, min_ceiling).unwrap_err();
    assert!(matches!(
        error,
        RuntimeError::AdmissionRejected {
            projected_bytes,
            maximum_bytes,
        } if projected_bytes == over_minimum && maximum_bytes == min_ceiling
    ));
    let message = error.to_string();
    assert!(message.contains(&over_minimum.to_string()), "{message}");
    assert!(message.contains(&min_ceiling.to_string()), "{message}");
}

#[test]
fn host_ceiling_scales_with_physical_memory_and_never_loosens_the_minimum_mac() {
    // Physical minus this host's OS reserve — max(8 GiB, 20%), the same rule
    // the exact accounting enforces — minus PAM's 1 GiB budget.
    assert_eq!(host_model_ceiling_bytes(MIN_HOST_BYTES), 24_696_061_952);
    assert_eq!(host_model_ceiling_bytes(OWNER_HOST_BYTES), 53_901_839_564);
    // The retired product-wide constant is the floor's upper bound: the
    // 32 GiB Mac gets no more headroom than it had.
    assert!(host_model_ceiling_bytes(MIN_HOST_BYTES) <= 27_000_000_000);
    // Above 40 GiB the 20% share exceeds the 8 GiB floor, so folding the
    // floor in changes nothing for larger Macs.
    assert_eq!(
        host_model_ceiling_bytes(OWNER_HOST_BYTES),
        OWNER_HOST_BYTES - OWNER_HOST_BYTES.div_ceil(5) - GIB
    );
    // A host that owes its whole capacity to the OS reserve admits nothing,
    // instead of advertising 5.8 GB it could never hold.
    assert_eq!(host_model_ceiling_bytes(8 * GIB), 0);
    assert_eq!(host_model_ceiling_bytes(GIB), 0);
    assert_eq!(host_model_ceiling_bytes(0), 0);
}

#[test]
fn uncalibrated_artifact_loads_when_it_fits_the_host_and_is_refused_when_it_does_not() {
    let large = registered(ContentDigest::from_sha256([3; 32]), LARGE_ARTIFACT_BYTES);

    // 64 GiB Mac: outside the calibrated set, inside the host ceiling.
    let owner_ceiling = host_model_ceiling_bytes(OWNER_HOST_BYTES);
    assert_eq!(
        artifact_calibration(&large, owner_ceiling).unwrap(),
        ArtifactCalibration::Uncalibrated
    );
    let contingency = calibrated_contingency(LARGE_ARTIFACT_BYTES);
    assert!(admit_bytes(LARGE_ARTIFACT_BYTES + contingency, owner_ceiling).is_ok());

    // 32 GiB Mac: refused, and the refusal names both numbers.
    let min_ceiling = host_model_ceiling_bytes(MIN_HOST_BYTES);
    let error = artifact_calibration(&large, min_ceiling).unwrap_err();
    assert!(matches!(
        error,
        RuntimeError::UnsupportedArtifact {
            size_bytes: LARGE_ARTIFACT_BYTES,
            maximum_bytes,
        } if maximum_bytes == min_ceiling
    ));
    let message = error.to_string();
    assert!(
        message.contains(&LARGE_ARTIFACT_BYTES.to_string()),
        "{message}"
    );
    assert!(message.contains(&min_ceiling.to_string()), "{message}");
    assert!(message.contains("calibrated set"), "{message}");

    // A small host refuses it too.
    assert!(matches!(
        artifact_calibration(&large, host_model_ceiling_bytes(8 * GIB)),
        Err(RuntimeError::UnsupportedArtifact { .. })
    ));
}

#[test]
fn calibrated_runtime_parameters_are_exact() {
    let model_params: LlamaModelParams = fixed_model_params();
    assert_eq!(model_params.n_gpu_layers(), i32::MAX);

    let context_params = fixed_context_params();
    assert_eq!(context_params.n_ctx().unwrap().get(), 8_192);
    assert_eq!(context_params.n_batch(), 512);
    assert_eq!(context_params.n_ubatch(), 512);
    assert_eq!(context_params.n_seq_max(), 1);
    assert_eq!(context_params.flash_attn_type(), LlamaFlashAttnType::Auto);
    assert_eq!(context_params.cache_type_k(), GgmlType::F16 as u32);
    assert_eq!(context_params.cache_type_v(), GgmlType::F16 as u32);
    assert!(!context_params.kv_unified());

    let digest = ContentDigest::parse(test_calibrated_digest()).unwrap();
    let projection = projection_from_entries(digest.clone(), [(1, 2, 3)]).unwrap();
    let profile = calibrated_runtime_profile(
        digest,
        ArtifactCalibration::Uncalibrated,
        host_model_ceiling_bytes(OWNER_HOST_BYTES),
        projection,
    )
    .unwrap();
    assert_eq!(profile.context_tokens(), 8_192);
    assert_eq!(profile.batch_tokens(), 512);
    assert_eq!(profile.physical_batch_tokens(), 512);
    assert_eq!(profile.parallel_sequences(), 1);
    assert_eq!(profile.gpu_offload(), RuntimeGpuOffload::All);
    assert_eq!(profile.flash_attention(), RuntimeFlashAttention::Auto);
    assert_eq!(profile.kv_cache_precision(), RuntimeKvCachePrecision::F16);
    assert!(!profile.kv_cache_unified());
    assert_eq!(profile.sampling(), RuntimeSampling::TopKTopPTemperature);
    assert_eq!(profile.max_projected_bytes(), 53_901_839_564);
    // An uncalibrated load is marked on the profile, never silently blessed.
    assert_eq!(profile.calibration(), ArtifactCalibration::Uncalibrated);
}

#[test]
fn projection_aggregates_all_entries_and_enforces_decimal_cap() {
    let digest = ContentDigest::parse(test_calibrated_digest()).unwrap();
    let projection = projection_from_entries(digest, [(10, 20, 30), (1, 2, 3)]).unwrap();
    assert_eq!(projection.weight_bytes(), 11);
    assert_eq!(projection.context_bytes(), 22);
    assert_eq!(projection.compute_bytes(), 33);
    assert!(admit_bytes(27_000_000_000, 27_000_000_000).is_ok());
    assert!(matches!(
        admit_bytes(27_000_000_001, 27_000_000_000),
        Err(RuntimeError::AdmissionRejected {
            projected_bytes: 27_000_000_001,
            maximum_bytes: 27_000_000_000,
        })
    ));
}

#[test]
fn calibrated_projection_contingency_rounds_up_to_five_percent() {
    assert_eq!(calibrated_contingency(18_587_496_448), 929_374_823);
    assert_eq!(calibrated_contingency(1), 256 * 1024 * 1024);
}

#[test]
fn prefill_is_bounded_and_sampling_slot_tracks_final_chunk() {
    assert_eq!(
        prefill_chunk_ranges(1_025).unwrap(),
        vec![0..512, 512..1_024, 1_024..1_025]
    );
    assert_eq!(final_prefill_sample_slot(512).unwrap(), 511);
    assert_eq!(final_prefill_sample_slot(513).unwrap(), 0);
    assert!(prefill_chunk_ranges(0).is_err());
    assert!(final_prefill_sample_slot(0).is_err());
}

#[test]
fn embedded_template_retry_is_strictly_bounded() {
    assert_eq!(bounded_template_size(4_097).unwrap(), 4_097);
    assert!(bounded_template_size(0).is_err());
    assert!(bounded_template_size(1024 * 1024 + 1).is_err());
}

#[test]
fn chat_messages_preserve_non_thinking_candidate_content_exactly() {
    let request = RuntimeRequest::new(
        vec![RuntimeMessage::new(RuntimeMessageRole::User, "analyze this").unwrap()],
        32,
    )
    .unwrap();
    assert_eq!(
        build_chat_messages(&request).unwrap(),
        vec![LlamaChatMessage::new("user".to_owned(), "analyze this".to_owned()).unwrap()]
    );
}

#[test]
fn host_admission_requires_fresh_normal_memory_facts() {
    let digest = ContentDigest::parse(test_calibrated_digest()).unwrap();
    let model = registered(digest.clone(), test_calibrated_size());
    let projection = projection_from_entries(digest, [(10, 20, 30)]).unwrap();
    let gib = GIB;
    let snapshot = RuntimeHostSnapshot::new(
        32 * gib,
        32 * gib,
        8 * gib,
        512 * 1024 * 1024,
        256 * 1024 * 1024,
        RuntimeMemoryPressure::Normal,
        RuntimeSwapTrend::Stable,
    )
    .unwrap();
    assert!(
        validate_host_admission(&model, &projection, snapshot, 20_000_000_000, 32 * gib,).is_ok()
    );
    let unknown_pressure = RuntimeHostSnapshot::new(
        32 * gib,
        32 * gib,
        8 * gib,
        512 * 1024 * 1024,
        256 * 1024 * 1024,
        RuntimeMemoryPressure::Unknown,
        RuntimeSwapTrend::Stable,
    )
    .unwrap();
    assert!(matches!(
        validate_host_admission(
            &model,
            &projection,
            unknown_pressure,
            20_000_000_000,
            32 * gib,
        ),
        Err(RuntimeError::AdmissionUnavailable(_))
    ));
    let rising_swap = RuntimeHostSnapshot::new(
        32 * gib,
        32 * gib,
        8 * gib,
        512 * 1024 * 1024,
        256 * 1024 * 1024,
        RuntimeMemoryPressure::Normal,
        RuntimeSwapTrend::Rising,
    )
    .unwrap();
    assert!(matches!(
        validate_host_admission(&model, &projection, rising_swap, 20_000_000_000, 32 * gib,),
        Err(RuntimeError::AdmissionUnavailable(_))
    ));
}

#[test]
fn memory_limits_require_one_positive_metal_and_one_positive_host_entry() {
    assert_eq!(
        metal_and_host_memory_limits([20_000_000_000, 32 * 1024 * 1024 * 1024]).unwrap(),
        (20_000_000_000, 32 * 1024 * 1024 * 1024)
    );
    for totals in [
        vec![],
        vec![20_000_000_000],
        vec![0, 32],
        vec![-1, 32],
        vec![1, 0],
        vec![1, -1],
        vec![1, 2, 3],
    ] {
        assert!(matches!(
            metal_and_host_memory_limits(totals),
            Err(RuntimeError::AdmissionUnavailable(_))
        ));
    }
}

#[test]
fn os_reserve_is_at_least_eight_gib_and_twenty_percent_physical() {
    let gib = GIB;
    assert_eq!(required_os_reserve(32 * gib), 8 * gib);
    assert_eq!(required_os_reserve(64 * gib), 13_743_895_348);
}

#[test]
fn host_admission_rejects_low_fractional_reserve_and_projection_mismatch() {
    let digest = ContentDigest::parse(test_calibrated_digest()).unwrap();
    let model = registered(digest.clone(), test_calibrated_size());
    let projection = projection_from_entries(digest, [(10, 20, 30)]).unwrap();
    let gib = 1024 * 1024 * 1024;
    let required = required_os_reserve(64 * gib);
    let low_reserve = RuntimeHostSnapshot::new(
        64 * gib,
        64 * gib,
        required - 1,
        512 * 1024 * 1024,
        256 * 1024 * 1024,
        RuntimeMemoryPressure::Normal,
        RuntimeSwapTrend::Stable,
    )
    .unwrap();
    assert!(matches!(
        validate_host_admission(&model, &projection, low_reserve, 20_000_000_000, 64 * gib,),
        Err(RuntimeError::AdmissionUnavailable(_))
    ));

    let valid_reserve = RuntimeHostSnapshot::new(
        64 * gib,
        64 * gib,
        required,
        512 * 1024 * 1024,
        256 * 1024 * 1024,
        RuntimeMemoryPressure::Normal,
        RuntimeSwapTrend::Stable,
    )
    .unwrap();
    assert!(
        validate_host_admission(&model, &projection, valid_reserve, 20_000_000_000, 64 * gib,)
            .is_ok()
    );
    assert!(matches!(
        validate_host_admission(&model, &projection, valid_reserve, 20_000_000_000, 32 * gib,),
        Err(RuntimeError::AdmissionUnavailable(_))
    ));
}

#[test]
fn host_snapshot_rejects_inconsistent_os_memory() {
    assert!(matches!(
        RuntimeHostSnapshot::new(
            1,
            2,
            0,
            0,
            0,
            RuntimeMemoryPressure::Normal,
            RuntimeSwapTrend::Stable,
        ),
        Err(RuntimeError::AdmissionUnavailable(_))
    ));
}

#[test]
fn finish_reason_remains_model_neutral() {
    assert_ne!(RuntimeFinishReason::Stop, RuntimeFinishReason::Length);
}

/// PAM's own largest calibrated artifact, the Q6\_K in `CALIBRATED_ARTIFACTS`.
const LARGEST_CALIBRATED_BYTES: u64 = 25_092_535_456;
/// Measured M4 Max Metal recommended maximum working set.
const METAL_WORKING_SET_BYTES: u64 = 55_662_788_608;
/// Context and compute bytes measured for the 8,192-token Qwen profile.
const PROFILE_CONTEXT_BYTES: usize = 805_306_368;
const PROFILE_COMPUTE_BYTES: usize = 315_359_232;

/// Exactly the snapshot the daemon's macOS adapter builds from `hw.memsize`:
/// every reserve derived from the host total, nothing fixed.
fn host_snapshot(total_bytes: u64) -> RuntimeHostSnapshot {
    RuntimeHostSnapshot::new(
        total_bytes,
        total_bytes,
        required_os_reserve(total_bytes),
        APPLICATION_RESERVE_BYTES,
        host_projection_contingency_bytes(total_bytes),
        RuntimeMemoryPressure::Normal,
        RuntimeSwapTrend::Stable,
    )
    .unwrap()
}

fn artifact_projection(digest: ContentDigest, size_bytes: u64) -> RuntimeMemoryProjection {
    projection_from_entries(
        digest,
        [(
            usize::try_from(size_bytes).unwrap(),
            PROFILE_CONTEXT_BYTES,
            PROFILE_COMPUTE_BYTES,
        )],
    )
    .unwrap()
}

#[test]
fn host_contingency_scales_with_physical_memory_and_never_loosens_the_minimum_mac() {
    // 5% of this host's ceiling: the largest projection any gate can admit.
    assert_eq!(
        host_projection_contingency_bytes(MIN_HOST_BYTES),
        1_234_803_098
    );
    assert_eq!(
        host_projection_contingency_bytes(OWNER_HOST_BYTES),
        2_695_091_979
    );
    // The retired fixed 1 GiB (1,073,741,824) stays the floor even after the
    // 8 GiB OS reserve was folded into the ceiling: the minimum Mac budgets
    // more contingency than the constant it replaced, not less.
    assert!(host_projection_contingency_bytes(MIN_HOST_BYTES) >= GIB);
    // The Q6_K no longer clears the 32 GiB ceiling at all, so its 5% is not
    // this host's problem; the 64 GiB host still covers the largest
    // uncalibrated artifact it can admit.
    assert!(
        host_projection_contingency_bytes(OWNER_HOST_BYTES)
            >= calibrated_contingency(LARGE_ARTIFACT_BYTES)
    );
    // A host too small to admit anything still gets the 256 MiB floor.
    assert_eq!(host_projection_contingency_bytes(GIB), 256 * 1024 * 1024);
    assert_eq!(
        host_projection_contingency_bytes(8 * GIB),
        256 * 1024 * 1024
    );
}

#[test]
fn contingency_gate_never_rejects_a_projection_the_ceiling_already_admitted() {
    for total in [8 * GIB, MIN_HOST_BYTES, OWNER_HOST_BYTES, 128 * GIB] {
        let ceiling = host_model_ceiling_bytes(total);
        assert!(host_projection_contingency_bytes(total) >= calibrated_contingency(ceiling));
    }
}

#[test]
fn host_derived_contingency_admits_the_owner_artifact_the_fixed_one_gib_rejected() {
    let digest = ContentDigest::parse(test_calibrated_digest()).unwrap();
    let owner = registered(digest.clone(), LARGE_ARTIFACT_BYTES);
    let projection = artifact_projection(digest, LARGE_ARTIFACT_BYTES);
    // Its 5% is nearly twice the retired fixed contingency.
    assert!(calibrated_contingency(LARGE_ARTIFACT_BYTES) > GIB);
    assert!(
        validate_host_admission(
            &owner,
            &projection,
            host_snapshot(OWNER_HOST_BYTES),
            METAL_WORKING_SET_BYTES,
            OWNER_HOST_BYTES,
        )
        .is_ok()
    );
    // A 32 GiB Mac still refuses it, and the refusal names both numbers.
    let error = artifact_calibration(&owner, host_model_ceiling_bytes(MIN_HOST_BYTES)).unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains(&LARGE_ARTIFACT_BYTES.to_string()),
        "{message}"
    );
    assert!(
        message.contains(&host_model_ceiling_bytes(MIN_HOST_BYTES).to_string()),
        "{message}"
    );
}

#[test]
fn largest_calibrated_artifact_is_refused_by_the_minimum_mac_ceiling_not_by_a_late_load_failure() {
    let digest = ContentDigest::parse(test_calibrated_digest()).unwrap();
    let model = registered(digest.clone(), LARGEST_CALIBRATED_BYTES);
    let projection = artifact_projection(digest, LARGEST_CALIBRATED_BYTES);
    let projected = LARGEST_CALIBRATED_BYTES
        + u64::try_from(PROFILE_CONTEXT_BYTES + PROFILE_COMPUTE_BYTES).unwrap();
    let admitted = projected + calibrated_contingency(projected);

    // 32 GiB: the ceiling now carries the absolute 8 GiB OS reserve, so the
    // gate that refuses this artifact is the ceiling itself, before load —
    // not the exact accounting discovering the same shortfall afterwards.
    let min_ceiling = host_model_ceiling_bytes(MIN_HOST_BYTES);
    let error = admit_bytes(admitted, min_ceiling).unwrap_err();
    assert!(matches!(
        error,
        RuntimeError::AdmissionRejected {
            projected_bytes,
            maximum_bytes,
        } if projected_bytes == admitted && maximum_bytes == min_ceiling
    ));
    // Weights alone are already over: no contingency or projection detail
    // can rescue 25.09 GB on a Mac that owes 8 GiB to the OS and 1 GiB to PAM.
    assert!(LARGEST_CALIBRATED_BYTES > min_ceiling);

    // A 64 GiB Mac admits the same artifact and projection end to end.
    assert!(admit_bytes(admitted, host_model_ceiling_bytes(OWNER_HOST_BYTES)).is_ok());
    assert!(
        validate_host_admission(
            &model,
            &projection,
            host_snapshot(OWNER_HOST_BYTES),
            METAL_WORKING_SET_BYTES,
            OWNER_HOST_BYTES,
        )
        .is_ok()
    );
}
