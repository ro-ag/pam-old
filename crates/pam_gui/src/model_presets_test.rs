use pam_model::ModelSource;

use crate::model_import::parse_model_key;
use crate::model_presets::{CATALOG, find};

const EXPECTED_IDS: [&str; 3] = [
    "qwen3-coder-30b-q4ks",
    "qwen3-coder-30b-q4km",
    "qwen3-coder-30b-q6k",
];

#[test]
fn catalog_has_exactly_the_three_curated_presets() {
    assert_eq!(CATALOG.len(), 3);
    let mut ids: Vec<&str> = CATALOG.iter().map(|preset| preset.id).collect();
    ids.sort_unstable();
    let mut expected = EXPECTED_IDS;
    expected.sort_unstable();
    assert_eq!(ids, expected);
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
fn min_memory_bytes_is_a_quarter_headroom_plus_two_gib() {
    const TWO_GIB: u64 = 2 << 30;
    for preset in CATALOG {
        let expected = preset.expected_size_bytes * 5 / 4 + TWO_GIB;
        assert_eq!(preset.min_memory_bytes(), expected, "{}", preset.id);
        assert!(preset.min_memory_bytes() > preset.expected_size_bytes);
    }
}

#[test]
fn find_looks_up_by_id_and_rejects_unknown_ids() {
    for id in EXPECTED_IDS {
        assert!(find(id).is_some());
    }
    assert!(find("does-not-exist").is_none());
}
