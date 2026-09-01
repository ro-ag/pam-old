use pam_model::{
    RuntimeHostAdmission, RuntimeMemoryPressure, RuntimeSwapTrend,
    host_projection_contingency_bytes, required_os_reserve,
};

use crate::macos_admission::{
    MacosRuntimeHostAdmission, parse_snapshot, parse_swapouts, swap_trend_from_samples,
};

const GIB: u64 = 1024 * 1024 * 1024;

#[test]
fn parses_sysctl_snapshot_and_applies_explicit_headroom() {
    let snapshot = parse_snapshot(b"34359738368\n75\n1\n", RuntimeSwapTrend::Stable).unwrap();

    assert_eq!(snapshot.total_bytes(), 32 * GIB);
    assert_eq!(snapshot.available_bytes(), 24 * GIB);
    assert_eq!(snapshot.reserved_os_bytes(), 8 * GIB);
    assert_eq!(snapshot.reserved_application_bytes(), GIB);
    // Host-derived, not a fixed 1 GiB: 5% of this Mac's model ceiling, which
    // is still above the retired constant after the absolute 8 GiB OS reserve
    // was folded into that ceiling. It covers every projection this Mac can
    // admit; Pam's 25,092,535,456-byte Q6_K is no longer one of them.
    assert_eq!(snapshot.projection_contingency_bytes(), 1_234_803_098);
    assert!(snapshot.projection_contingency_bytes() > GIB);
    assert_eq!(snapshot.pressure(), RuntimeMemoryPressure::Normal);
    assert_eq!(snapshot.swap_trend(), RuntimeSwapTrend::Stable);
}

#[test]
fn every_snapshot_reserve_is_derived_from_this_host() {
    for (total, contingency) in [
        (34_359_738_368_u64, 1_234_803_098_u64),
        (68_719_476_736, 2_695_091_979),
    ] {
        let input = format!("{total}\n100\n1\n");
        let snapshot = parse_snapshot(input.as_bytes(), RuntimeSwapTrend::Stable).unwrap();
        assert_eq!(snapshot.reserved_os_bytes(), required_os_reserve(total));
        assert_eq!(snapshot.projection_contingency_bytes(), contingency);
        assert_eq!(
            snapshot.projection_contingency_bytes(),
            host_projection_contingency_bytes(total)
        );
    }
}

#[test]
fn maps_pressure_levels_and_preserves_unknown_values() {
    for (level, expected) in [
        ("1", RuntimeMemoryPressure::Normal),
        ("2", RuntimeMemoryPressure::Warning),
        ("4", RuntimeMemoryPressure::Critical),
        ("0", RuntimeMemoryPressure::Unknown),
        ("8", RuntimeMemoryPressure::Unknown),
    ] {
        let input = format!("34359738368\n80\n{level}\n");
        assert_eq!(
            parse_snapshot(input.as_bytes(), RuntimeSwapTrend::Stable)
                .unwrap()
                .pressure(),
            expected
        );
    }
}

#[test]
fn rejects_malformed_or_ambiguous_snapshots() {
    for input in [
        b"".as_slice(),
        b"0\n50\n1\n",
        b"34359738368\n101\n1\n",
        b"34359738368\n50\n",
        b"34359738368\n50\n1\nextra\n",
        b"34359738368\nnope\n1\n",
        b"34359738368\n+50\n1\n",
    ] {
        assert!(parse_snapshot(input, RuntimeSwapTrend::Stable).is_err());
    }
}

#[test]
fn parses_exact_vm_stat_swapout_counter_and_rejects_ambiguity() {
    let valid = b"Mach Virtual Memory Statistics: (page size of 16384 bytes)\nPages free: 1.\nSwapouts: 43160.\n";
    assert_eq!(parse_swapouts(valid).unwrap(), 43_160);
    for invalid in [
        b"".as_slice(),
        b"Swapouts: 1\n",
        b"Swapouts: +1.\n",
        b"Swapouts: one.\n",
        b"Swapouts: 1.\nSwapouts: 1.\n",
        b"swapouts: 1.\n",
    ] {
        assert!(parse_swapouts(invalid).is_err());
    }
}

#[test]
fn swapout_samples_fail_closed_unless_the_counter_is_stable() {
    assert_eq!(swap_trend_from_samples(7, 7), RuntimeSwapTrend::Stable);
    assert_eq!(swap_trend_from_samples(7, 8), RuntimeSwapTrend::Rising);
    assert_eq!(swap_trend_from_samples(8, 7), RuntimeSwapTrend::Unknown);
}

#[test]
fn live_snapshot_uses_the_fail_closed_platform_query() {
    let snapshot = MacosRuntimeHostAdmission.snapshot().unwrap();

    assert!(snapshot.total_bytes() > 0);
    assert!(snapshot.available_bytes() <= snapshot.total_bytes());
    assert!(snapshot.reserved_os_bytes() >= 8 * GIB);
    assert!(snapshot.projection_contingency_bytes() >= GIB);
}
