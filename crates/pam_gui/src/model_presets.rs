//! Curated, pre-verified model download presets.
//!
//! Every entry here is static data checked in by hand against the vendor's
//! published artifact; PAM never fetches or infers preset metadata over the
//! network. The GUI downloads and registers a preset through the
//! `model_download` runtime, never a bare user-supplied URL.
//!
//! A preset carries its own size and digest literals. Membership in
//! [`pam_model::CALIBRATED_ARTIFACTS`] — the measured, known-good set — is a
//! separate verdict PAM surfaces per preset, not a precondition for offering
//! one.

use pam_core::ContentDigest;
use pam_model::{LicenseSnapshot, ModelError, is_calibrated_artifact};

use crate::model_import::notice_digest;

/// One curated, pre-verified downloadable model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModelPreset {
    pub id: &'static str,
    pub label: &'static str,
    /// Stable `vendor/name` model identity.
    pub model: &'static str,
    pub file_name: &'static str,
    pub url: &'static str,
    pub expected_size_bytes: u64,
    /// Lowercase hex SHA-256, without the `sha256:` prefix.
    pub sha256: &'static str,
    pub license_id: &'static str,
    pub license_url: &'static str,
    /// The exact notice text a user accepts on screen; its digest becomes the
    /// license snapshot's notice digest at registration.
    pub license_notice_text: &'static str,
    pub params_label: &'static str,
    pub quant_label: &'static str,
}

impl ModelPreset {
    /// Parses the plain hex digest into PAM's canonical `sha256:` form.
    ///
    /// # Panics
    ///
    /// Never for [`CATALOG`] entries: `catalog_digests_are_valid_sha256`
    /// asserts every catalog digest parses.
    #[must_use]
    pub fn expected_digest(&self) -> ContentDigest {
        ContentDigest::parse(format!("sha256:{}", self.sha256))
            .expect("catalog digest is a validated 64-hex-char sha256")
    }

    /// True when this exact artifact is in PAM's measured, known-good set.
    /// A false verdict is not a refusal — the runtime loads uncalibrated
    /// artifacts that fit — but the picker says so before tens of GB move.
    #[must_use]
    pub fn calibrated(&self) -> bool {
        is_calibrated_artifact(self.sha256, self.expected_size_bytes)
    }

    /// Whether a host with `host_total_bytes` of physical memory can run this
    /// preset, by the same rule the macOS runtime admits an artifact with.
    #[must_use]
    pub fn fits_host(&self, host_total_bytes: u64) -> bool {
        self.expected_size_bytes <= host_model_budget_bytes(host_total_bytes)
    }

    /// Builds the license snapshot a user accepts for this preset.
    ///
    /// # Errors
    ///
    /// Returns [`ModelError::InvalidLicense`] when the catalog's license
    /// identifier or URL is malformed.
    pub(crate) fn license(&self) -> Result<LicenseSnapshot, ModelError> {
        LicenseSnapshot::new(
            self.license_id,
            self.license_url,
            notice_digest(self.license_notice_text),
        )
    }
}

/// The largest artifact this host can devote to a model: its runtime ceiling
/// (physical total less the OS reserve and PAM's own application reserve)
/// minus the projection contingency every admitted load must budget for.
///
/// This is the picker's fit rule, and it is the daemon's own arithmetic —
/// `size + host_projection_contingency_bytes(total) <= host_model_ceiling_bytes(total)`
/// rearranged into one number — so the GUI stops offering artifacts the
/// runtime will refuse at load. Advisory still: the daemon re-checks against
/// a live snapshot (availability, pressure, Metal working set) at load time.
#[cfg(target_os = "macos")]
#[must_use]
pub fn host_model_budget_bytes(host_total_bytes: u64) -> u64 {
    pam_model::host_model_ceiling_bytes(host_total_bytes).saturating_sub(
        pam_model::host_projection_contingency_bytes(host_total_bytes),
    )
}

/// PAM's model runtime is macOS-only, and so is the host memory probe that
/// feeds this; off macOS nothing is ever measured, so nothing is refused.
#[cfg(not(target_os = "macos"))]
#[must_use]
pub fn host_model_budget_bytes(_host_total_bytes: u64) -> u64 {
    u64::MAX
}

/// Static curated catalog: exactly the presets PAM offers today.
///
/// Tiered by quantization from a 32 GiB Mac up to a 128 GiB one. The floor is
/// the 24B/30B class — PAM's jobs (git history, Sonar logs, evidence) need
/// real capacity, so nothing smaller belongs here. Which tiers a given Mac
/// can actually run is [`ModelPreset::fits_host`]; the rest are shown
/// disabled, with the reason.
///
/// ponytail: one preset is one file, one digest, one size. Sharded GGUF
/// releases (GLM-4.5-Air's usable quants, for one) cannot be expressed here
/// at all; multi-part download and verification would have to come first.
pub const CATALOG: &[ModelPreset] = &[
    ModelPreset {
        id: "qwen3-coder-30b-q4ks",
        label: "Qwen3 Coder 30B — minimum",
        model: "qwen/qwen3-coder-30b-a3b-instruct-q4_k_s",
        file_name: "Qwen3-Coder-30B-A3B-Instruct-Q4_K_S.gguf",
        url: "https://huggingface.co/unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF/resolve/main/Qwen3-Coder-30B-A3B-Instruct-Q4_K_S.gguf",
        expected_size_bytes: 17_456_012_448,
        sha256: "56a7d00783419bcb0ae566253c371bcb3678261bb79881a553539f5679864db4",
        license_id: "Apache-2.0",
        license_url: "https://www.apache.org/licenses/LICENSE-2.0",
        license_notice_text: "Qwen3-Coder-30B-A3B-Instruct-Q4_K_S.gguf is distributed under the Apache-2.0 license at https://www.apache.org/licenses/LICENSE-2.0.",
        params_label: "30B-A3B",
        quant_label: "Q4_K_S",
    },
    ModelPreset {
        id: "qwen3-coder-30b-q4km",
        label: "Qwen3 Coder 30B — balanced",
        model: "qwen/qwen3-coder-30b-a3b-instruct-q4_k_m",
        file_name: "Qwen3-Coder-30B-A3B-Instruct-Q4_K_M.gguf",
        url: "https://huggingface.co/unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF/resolve/main/Qwen3-Coder-30B-A3B-Instruct-Q4_K_M.gguf",
        expected_size_bytes: 18_556_689_568,
        sha256: "fadc3e5f8d42bf7e894a785b05082e47daee4df26680389817e2093056f088ad",
        license_id: "Apache-2.0",
        license_url: "https://www.apache.org/licenses/LICENSE-2.0",
        license_notice_text: "Qwen3-Coder-30B-A3B-Instruct-Q4_K_M.gguf is distributed under the Apache-2.0 license at https://www.apache.org/licenses/LICENSE-2.0.",
        params_label: "30B-A3B",
        quant_label: "Q4_K_M",
    },
    ModelPreset {
        id: "qwen3-coder-30b-q5km",
        label: "Qwen3 Coder 30B — refined",
        model: "qwen/qwen3-coder-30b-a3b-instruct-q5_k_m",
        file_name: "Qwen3-Coder-30B-A3B-Instruct-Q5_K_M.gguf",
        url: "https://huggingface.co/unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF/resolve/main/Qwen3-Coder-30B-A3B-Instruct-Q5_K_M.gguf",
        expected_size_bytes: 21_725_584_544,
        sha256: "4b78837bbec5ee248e4a5642bf608b6793721af41b92589e40c8da0bce58b907",
        license_id: "Apache-2.0",
        license_url: "https://www.apache.org/licenses/LICENSE-2.0",
        license_notice_text: "Qwen3-Coder-30B-A3B-Instruct-Q5_K_M.gguf is distributed under the Apache-2.0 license at https://www.apache.org/licenses/LICENSE-2.0.",
        params_label: "30B-A3B",
        quant_label: "Q5_K_M",
    },
    ModelPreset {
        id: "qwen3-coder-30b-q6k",
        label: "Qwen3 Coder 30B — high fidelity",
        model: "qwen/qwen3-coder-30b-a3b-instruct-q6_k",
        file_name: "Qwen3-Coder-30B-A3B-Instruct-Q6_K.gguf",
        url: "https://huggingface.co/unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF/resolve/main/Qwen3-Coder-30B-A3B-Instruct-Q6_K.gguf",
        expected_size_bytes: 25_092_535_456,
        sha256: "100b5121d09553fb1af3b873b21fb3ec3da5c306fc5cb09bd338c48e21b10875",
        license_id: "Apache-2.0",
        license_url: "https://www.apache.org/licenses/LICENSE-2.0",
        license_notice_text: "Qwen3-Coder-30B-A3B-Instruct-Q6_K.gguf is distributed under the Apache-2.0 license at https://www.apache.org/licenses/LICENSE-2.0.",
        params_label: "30B-A3B",
        quant_label: "Q6_K",
    },
    ModelPreset {
        id: "qwen3-coder-30b-q80",
        label: "Qwen3 Coder 30B — maximum fidelity",
        model: "qwen/qwen3-coder-30b-a3b-instruct-q8_0",
        file_name: "Qwen3-Coder-30B-A3B-Instruct-Q8_0.gguf",
        url: "https://huggingface.co/unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF/resolve/main/Qwen3-Coder-30B-A3B-Instruct-Q8_0.gguf",
        expected_size_bytes: 32_483_935_392,
        sha256: "4ff1cff607804037bf6d2168249c570baa4e1621292b159c0e06591e0d7c3066",
        license_id: "Apache-2.0",
        license_url: "https://www.apache.org/licenses/LICENSE-2.0",
        license_notice_text: "Qwen3-Coder-30B-A3B-Instruct-Q8_0.gguf is distributed under the Apache-2.0 license at https://www.apache.org/licenses/LICENSE-2.0.",
        params_label: "30B-A3B",
        quant_label: "Q8_0",
    },
    ModelPreset {
        id: "devstral-small-2-24b-q4km",
        label: "Devstral Small 2 24B — balanced",
        model: "mistral/devstral-small-2-24b-instruct-2512-q4_k_m",
        file_name: "Devstral-Small-2-24B-Instruct-2512-Q4_K_M.gguf",
        url: "https://huggingface.co/unsloth/Devstral-Small-2-24B-Instruct-2512-GGUF/resolve/main/Devstral-Small-2-24B-Instruct-2512-Q4_K_M.gguf",
        expected_size_bytes: 14_334_446_752,
        sha256: "d14ba9edee1bb4c4996a726deb81e49ae81800a3216f0774634238c380aee496",
        license_id: "Apache-2.0",
        license_url: "https://www.apache.org/licenses/LICENSE-2.0",
        license_notice_text: "Devstral-Small-2-24B-Instruct-2512-Q4_K_M.gguf is distributed under the Apache-2.0 license at https://www.apache.org/licenses/LICENSE-2.0.",
        params_label: "24B",
        quant_label: "Q4_K_M",
    },
    ModelPreset {
        id: "devstral-small-2-24b-q5km",
        label: "Devstral Small 2 24B — refined",
        model: "mistral/devstral-small-2-24b-instruct-2512-q5_k_m",
        file_name: "Devstral-Small-2-24B-Instruct-2512-Q5_K_M.gguf",
        url: "https://huggingface.co/unsloth/Devstral-Small-2-24B-Instruct-2512-GGUF/resolve/main/Devstral-Small-2-24B-Instruct-2512-Q5_K_M.gguf",
        expected_size_bytes: 16_764_521_632,
        sha256: "2da6ca6c4ae387aa7f3f2f4a67bb3e1ca570ce0c69c21e26b8695e75172443b0",
        license_id: "Apache-2.0",
        license_url: "https://www.apache.org/licenses/LICENSE-2.0",
        license_notice_text: "Devstral-Small-2-24B-Instruct-2512-Q5_K_M.gguf is distributed under the Apache-2.0 license at https://www.apache.org/licenses/LICENSE-2.0.",
        params_label: "24B",
        quant_label: "Q5_K_M",
    },
    ModelPreset {
        id: "devstral-small-2-24b-q6k",
        label: "Devstral Small 2 24B — high fidelity",
        model: "mistral/devstral-small-2-24b-instruct-2512-q6_k",
        file_name: "Devstral-Small-2-24B-Instruct-2512-Q6_K.gguf",
        url: "https://huggingface.co/unsloth/Devstral-Small-2-24B-Instruct-2512-GGUF/resolve/main/Devstral-Small-2-24B-Instruct-2512-Q6_K.gguf",
        expected_size_bytes: 19_346_476_192,
        sha256: "b04b3e19730d7a1e19530f40b947b69c028b090cb7c58c1515cf1fc2ece5f821",
        license_id: "Apache-2.0",
        license_url: "https://www.apache.org/licenses/LICENSE-2.0",
        license_notice_text: "Devstral-Small-2-24B-Instruct-2512-Q6_K.gguf is distributed under the Apache-2.0 license at https://www.apache.org/licenses/LICENSE-2.0.",
        params_label: "24B",
        quant_label: "Q6_K",
    },
    ModelPreset {
        id: "devstral-small-2-24b-q80",
        label: "Devstral Small 2 24B — maximum fidelity",
        model: "mistral/devstral-small-2-24b-instruct-2512-q8_0",
        file_name: "Devstral-Small-2-24B-Instruct-2512-Q8_0.gguf",
        url: "https://huggingface.co/unsloth/Devstral-Small-2-24B-Instruct-2512-GGUF/resolve/main/Devstral-Small-2-24B-Instruct-2512-Q8_0.gguf",
        expected_size_bytes: 25_055_317_152,
        sha256: "0760502e9228234f6cfa843f8870b8fc91c46a13664cf766c639229cccc80866",
        license_id: "Apache-2.0",
        license_url: "https://www.apache.org/licenses/LICENSE-2.0",
        license_notice_text: "Devstral-Small-2-24B-Instruct-2512-Q8_0.gguf is distributed under the Apache-2.0 license at https://www.apache.org/licenses/LICENSE-2.0.",
        params_label: "24B",
        quant_label: "Q8_0",
    },
    ModelPreset {
        id: "devstral-small-2-24b-bf16",
        label: "Devstral Small 2 24B — full precision",
        model: "mistral/devstral-small-2-24b-instruct-2512-bf16",
        file_name: "Devstral-Small-2-24B-Instruct-2512-BF16.gguf",
        url: "https://huggingface.co/unsloth/Devstral-Small-2-24B-Instruct-2512-GGUF/resolve/main/Devstral-Small-2-24B-Instruct-2512-BF16.gguf",
        expected_size_bytes: 47_154_056_032,
        sha256: "6a86365cc26ec2e5ba1434aa85da15a7de28eb92015447623a780f1f86ab1d1b",
        license_id: "Apache-2.0",
        license_url: "https://www.apache.org/licenses/LICENSE-2.0",
        license_notice_text: "Devstral-Small-2-24B-Instruct-2512-BF16.gguf is distributed under the Apache-2.0 license at https://www.apache.org/licenses/LICENSE-2.0.",
        params_label: "24B",
        quant_label: "BF16",
    },
    ModelPreset {
        id: "gpt-oss-120b-f16",
        label: "GPT-OSS 120B — full precision",
        model: "openai/gpt-oss-120b-f16",
        file_name: "gpt-oss-120b-F16.gguf",
        url: "https://huggingface.co/unsloth/gpt-oss-120b-GGUF/resolve/main/gpt-oss-120b-F16.gguf",
        expected_size_bytes: 65_369_017_728,
        sha256: "2d1f0298ae4b6c874d5a468598c5ce17c1763b3fea99de10b1a07df93cef014f",
        license_id: "Apache-2.0",
        license_url: "https://www.apache.org/licenses/LICENSE-2.0",
        license_notice_text: "gpt-oss-120b-F16.gguf is distributed under the Apache-2.0 license at https://www.apache.org/licenses/LICENSE-2.0.",
        params_label: "120B",
        quant_label: "F16",
    },
];

/// Looks up one catalog preset by its stable id.
#[must_use]
pub fn find(id: &str) -> Option<&'static ModelPreset> {
    CATALOG.iter().find(|preset| preset.id == id)
}
