use std::time::{Duration, Instant};

use crate::patience::{classify_deadline, process_cpu_time};

#[cfg(unix)]
#[test]
fn cpu_sampler_reads_and_never_decreases() {
    let before = process_cpu_time().expect("unix process CPU time should be readable");

    // Burn a little CPU so the second sample has something to move on.
    let spin_start = Instant::now();
    let mut accumulator = 0_u64;
    while spin_start.elapsed() < Duration::from_millis(20) {
        accumulator = std::hint::black_box(accumulator.wrapping_mul(31).wrapping_add(7));
    }

    let after = process_cpu_time().expect("unix process CPU time should stay readable");
    assert!(
        after >= before,
        "process CPU time went backwards: {before:?} then {after:?}"
    );
}

#[test]
fn low_cpu_across_the_wait_classifies_as_starved() {
    let line = classify_deadline(Duration::from_secs(10), Some(Duration::from_millis(250)))
        .expect("a CPU reading must produce a marker");
    assert_eq!(
        line,
        "PAM-TIMEOUT-STARVED: exchange deadline after 10.0s with 0.25s process CPU — consistent with runner starvation"
    );
}

#[test]
fn cpu_matching_the_wait_classifies_as_engaged() {
    let line = classify_deadline(Duration::from_secs(2), Some(Duration::from_secs(2)))
        .expect("a CPU reading must produce a marker");
    assert_eq!(
        line,
        "PAM-TIMEOUT-ENGAGED: exchange deadline after 2.0s with 2.00s process CPU — the process was running; treat as a defect"
    );
}

#[test]
fn cpu_exceeding_the_wait_classifies_as_engaged() {
    let line = classify_deadline(Duration::from_secs(3), Some(Duration::from_secs(4)))
        .expect("a CPU reading must produce a marker");
    assert!(line.starts_with("PAM-TIMEOUT-ENGAGED: "), "got: {line}");
}

#[test]
fn unreadable_cpu_produces_no_marker() {
    assert_eq!(classify_deadline(Duration::from_secs(30), None), None);
}
