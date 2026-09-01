use pam_model::{ModelSource, is_calibrated_artifact};

use crate::model_import::parse_model_key;
use crate::model_presets::{CATALOG, find};

/// The measured, known-good entries: these three are in
/// `pam_model::CALIBRATED_ARTIFACTS`. Everything else in the catalog is
/// offered as uncalibrated.
const CALIBRATED_IDS: [&str; 3] = [
    "qwen3-coder-30b-q4ks",
    "qwen3-coder-30b-q4km",
    "qwen3-coder-30b-q6k",
];

/// Pam's floor is the 24B/30B class, so no catalog artifact is small; the
/// upper bound is a sanity rail against a mistyped literal.
const MIN_PRESET_BYTES: u64 = 10 << 30;
const MAX_PRESET_BYTES: u64 = 80 << 30;

#[test]
fn every_catalog_entry_is_well_formed() {
    assert!(!CATALOG.is_empty());
    for preset in CATALOG {
        assert!(!preset.id.is_empty(), "{}", preset.id);
        assert!(!preset.label.is_empty(), "{}", preset.id);
        assert!(
            (MIN_PRESET_BYTES..=MAX_PRESET_BYTES).contains(&preset.expected_size_bytes),
            "{} has an implausible size",
            preset.id
        );
        assert!(
            pam_model::validate_model_filename(preset.file_name).is_ok(),
            "{} has an invalid filename",
            preset.id
        );
        assert!(
            preset.url.ends_with(preset.file_name),
            "{} does not download the file it names",
            preset.id
        );
        assert!(
            preset.license_notice_text.contains(preset.file_name)
                && preset.license_notice_text.contains(preset.license_url),
            "{} has a notice that does not name its file and licence",
            preset.id
        );
        assert!(!preset.params_label.is_empty(), "{}", preset.id);
        assert!(!preset.quant_label.is_empty(), "{}", preset.id);
    }
}

#[test]
fn catalog_ids_are_unique() {
    let mut ids: Vec<&str> = CATALOG.iter().map(|preset| preset.id).collect();
    let before = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), before);
}

#[test]
fn catalog_digests_are_valid_lowercase_sha256_hex() {
    for preset in CATALOG {
        assert_eq!(preset.sha256.len(), 64, "{}", preset.id);
        assert!(
            preset
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')),
            "{} has a non-lowercase-hex digest",
            preset.id
        );
        // Panics (failing the test) if the digest does not parse.
        let _ = preset.expected_digest();
    }
}

/// The catalog is no longer welded to the calibrated set: a preset carries
/// its own size and digest, and `CALIBRATED_ARTIFACTS` stays the *measured*
/// set. Exactly the three original entries are measured; every later one is
/// flagged uncalibrated so the picker can say so before tens of GB move.
#[test]
fn exactly_the_three_original_entries_are_calibrated() {
    let mut calibrated: Vec<&str> = CATALOG
        .iter()
        .filter(|preset| preset.calibrated())
        .map(|preset| preset.id)
        .collect();
    calibrated.sort_unstable();
    let mut expected = CALIBRATED_IDS;
    expected.sort_unstable();
    assert_eq!(calibrated, expected);

    for preset in CATALOG {
        assert_eq!(
            preset.calibrated(),
            is_calibrated_artifact(preset.sha256, preset.expected_size_bytes),
            "{} reports a calibration verdict the runtime would not agree with",
            preset.id
        );
    }
}

#[test]
fn catalog_urls_are_safe_https_sources() {
    for preset in CATALOG {
        assert!(
            ModelSource::https(preset.url).is_ok(),
            "{} has an unsafe source URL",
            preset.id
        );
    }
}

#[test]
fn catalog_licenses_build_with_safe_https_notice_urls() {
    for preset in CATALOG {
        assert!(
            preset.license().is_ok(),
            "{} has an invalid license",
            preset.id
        );
    }
}

#[test]
fn catalog_model_identities_parse_as_vendor_name_pairs() {
    for preset in CATALOG {
        assert!(
            parse_model_key(preset.model).is_ok(),
            "{} has an invalid model identity",
            preset.id
        );
    }
}

#[test]
fn find_looks_up_by_id_and_rejects_unknown_ids() {
    for preset in CATALOG {
        assert_eq!(find(preset.id), Some(preset));
    }
    assert!(find("does-not-exist").is_none());
}

#[cfg(target_os = "macos")]
mod host_fit {
    use pam_model::{host_model_ceiling_bytes, host_projection_contingency_bytes};

    use crate::model_presets::{CATALOG, find, host_model_budget_bytes};

    const GIB: u64 = 1 << 30;
    /// The host sizes Pam tiers the catalog across: the supported minimum up
    /// to a 128 GiB Mac.
    const HOSTS: [u64; 5] = [32 * GIB, 48 * GIB, 64 * GIB, 96 * GIB, 128 * GIB];

    /// `(id, fits 32 / 48 / 64 / 96 / 128 GiB)` — the tiering the picker
    /// shows, pinned as literals so a size or rule change has to be
    /// deliberate.
    const EXPECTED_FIT: [(&str, [bool; 5]); 11] = [
        ("qwen3-coder-30b-q4ks", [true, true, true, true, true]),
        ("qwen3-coder-30b-q4km", [true, true, true, true, true]),
        ("qwen3-coder-30b-q5km", [true, true, true, true, true]),
        ("qwen3-coder-30b-q6k", [false, true, true, true, true]),
        ("qwen3-coder-30b-q80", [false, true, true, true, true]),
        ("devstral-small-2-24b-q4km", [true, true, true, true, true]),
        ("devstral-small-2-24b-q5km", [true, true, true, true, true]),
        ("devstral-small-2-24b-q6k", [true, true, true, true, true]),
        ("devstral-small-2-24b-q80", [false, true, true, true, true]),
        (
            "devstral-small-2-24b-bf16",
            [false, false, true, true, true],
        ),
        ("gpt-oss-120b-f16", [false, false, false, true, true]),
    ];

    /// The budget is the daemon's own admission arithmetic, rearranged:
    /// `size + contingency(total) <= ceiling(total)`.
    #[test]
    fn budget_is_the_host_ceiling_less_its_projection_contingency() {
        for total in HOSTS {
            assert_eq!(
                host_model_budget_bytes(total),
                host_model_ceiling_bytes(total) - host_projection_contingency_bytes(total)
            );
        }
        // A host below Pam's minimum has no ceiling at all, so nothing fits.
        assert_eq!(host_model_budget_bytes(8 * GIB), 0);
    }

    #[test]
    fn every_preset_is_tiered_by_host_memory() {
        assert_eq!(EXPECTED_FIT.len(), CATALOG.len());
        for (id, expected) in EXPECTED_FIT {
            let preset = find(id).unwrap_or_else(|| panic!("{id} left the catalog"));
            for (total, fits) in HOSTS.into_iter().zip(expected) {
                assert_eq!(preset.fits_host(total), fits, "{id} on a {total}-byte host");
            }
        }
    }

    /// Every host size has something to run, and the largest tier is only
    /// reachable on the largest Macs.
    #[test]
    fn each_host_size_can_run_something_and_the_top_tier_needs_96_gib() {
        for total in HOSTS {
            assert!(
                CATALOG.iter().any(|preset| preset.fits_host(total)),
                "nothing fits a {total}-byte host"
            );
        }
        let biggest = find("gpt-oss-120b-f16").unwrap();
        assert!(!biggest.fits_host(64 * GIB));
        assert!(biggest.fits_host(96 * GIB));
    }

    /// The bug this rule replaced: the old `size * 5/4 + 2 GiB` floor was
    /// compared against *raw* physical memory, so a 32 GiB Mac was offered
    /// quants the daemon then refused at load.
    #[test]
    fn a_32_gib_mac_is_not_offered_quants_the_daemon_would_refuse() {
        let host_32 = 32 * GIB;
        for id in ["qwen3-coder-30b-q6k", "qwen3-coder-30b-q80"] {
            let preset = find(id).unwrap();
            assert!(!preset.fits_host(host_32), "{id}");
            assert!(
                preset.expected_size_bytes + host_projection_contingency_bytes(host_32)
                    > host_model_ceiling_bytes(host_32),
                "{id}"
            );
        }
        for id in [
            "devstral-small-2-24b-q4km",
            "devstral-small-2-24b-q5km",
            "devstral-small-2-24b-q6k",
        ] {
            assert!(find(id).unwrap().fits_host(host_32), "{id}");
        }
    }
}
