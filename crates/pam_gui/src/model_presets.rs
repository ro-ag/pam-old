//! Curated, pre-verified model download presets.
//!
//! Every entry here is static data checked in by hand against the vendor's
//! published artifact; PAM never fetches or infers preset metadata over the
//! network. The GUI downloads and registers a preset through the
//! `model_download` runtime, never a bare user-supplied URL.

use pam_core::ContentDigest;
use pam_model::{LicenseSnapshot, ModelError};

use crate::model_import::notice_digest;

const GIB: u64 = 1 << 30;

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

    /// A coarse working-set floor: file size plus 25% headroom plus a fixed
    /// 2 GiB OS/runtime cushion.
    ///
    /// ponytail: linear heuristic, not a real memory projection — the
    /// daemon's llama.cpp estimate at load time stays authoritative.
    #[must_use]
    pub const fn min_memory_bytes(&self) -> u64 {
        self.expected_size_bytes * 5 / 4 + 2 * GIB
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

/// Static curated catalog: exactly the presets PAM offers today.
///
/// Qwen3-Coder-30B-A3B-Instruct at `Q4_K_S` is the smallest model PAM's flows
/// were validated on — the quality floor. Larger quants of the same model are
/// the upgrades; nothing smaller belongs in this catalog.
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
];

/// Looks up one catalog preset by its stable id.
#[must_use]
pub fn find(id: &str) -> Option<&'static ModelPreset> {
    CATALOG.iter().find(|preset| preset.id == id)
}
