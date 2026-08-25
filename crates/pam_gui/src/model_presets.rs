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
pub const CATALOG: &[ModelPreset] = &[
    ModelPreset {
        id: "qwen3-8b-q4km",
        label: "Qwen3 8B",
        model: "qwen/qwen3-8b-q4_k_m",
        file_name: "Qwen3-8B-Q4_K_M.gguf",
        url: "https://huggingface.co/Qwen/Qwen3-8B-GGUF/resolve/main/Qwen3-8B-Q4_K_M.gguf",
        expected_size_bytes: 5_027_783_488,
        sha256: "d98cdcbd03e17ce47681435b5150e34c1417f50b5c0019dd560e4882c5745785",
        license_id: "Apache-2.0",
        license_url: "https://www.apache.org/licenses/LICENSE-2.0",
        license_notice_text: "Qwen3-8B-Q4_K_M.gguf is distributed under the Apache-2.0 license at https://www.apache.org/licenses/LICENSE-2.0.",
        params_label: "8B",
        quant_label: "Q4_K_M",
    },
    ModelPreset {
        id: "qwen3-14b-q4km",
        label: "Qwen3 14B",
        model: "qwen/qwen3-14b-q4_k_m",
        file_name: "Qwen3-14B-Q4_K_M.gguf",
        url: "https://huggingface.co/Qwen/Qwen3-14B-GGUF/resolve/main/Qwen3-14B-Q4_K_M.gguf",
        expected_size_bytes: 9_001_752_960,
        sha256: "500a8806e85ee9c83f3ae08420295592451379b4f8cf2d0f41c15dffeb6b81f0",
        license_id: "Apache-2.0",
        license_url: "https://www.apache.org/licenses/LICENSE-2.0",
        license_notice_text: "Qwen3-14B-Q4_K_M.gguf is distributed under the Apache-2.0 license at https://www.apache.org/licenses/LICENSE-2.0.",
        params_label: "14B",
        quant_label: "Q4_K_M",
    },
    ModelPreset {
        id: "llama31-8b-q4km",
        label: "Llama 3.1 8B Instruct",
        model: "meta/llama-3.1-8b-instruct-q4_k_m",
        file_name: "Meta-Llama-3.1-8B-Instruct-Q4_K_M.gguf",
        url: "https://huggingface.co/bartowski/Meta-Llama-3.1-8B-Instruct-GGUF/resolve/main/Meta-Llama-3.1-8B-Instruct-Q4_K_M.gguf",
        expected_size_bytes: 4_920_739_232,
        sha256: "7b064f5842bf9532c91456deda288a1b672397a54fa729aa665952863033557c",
        license_id: "Llama-3.1-Community-License",
        license_url: "https://huggingface.co/meta-llama/Llama-3.1-8B-Instruct/blob/main/LICENSE",
        license_notice_text: "Meta-Llama-3.1-8B-Instruct-Q4_K_M.gguf is distributed under the Llama-3.1-Community-License license at https://huggingface.co/meta-llama/Llama-3.1-8B-Instruct/blob/main/LICENSE.",
        params_label: "8B",
        quant_label: "Q4_K_M",
    },
];

/// Looks up one catalog preset by its stable id.
#[must_use]
pub fn find(id: &str) -> Option<&'static ModelPreset> {
    CATALOG.iter().find(|preset| preset.id == id)
}
