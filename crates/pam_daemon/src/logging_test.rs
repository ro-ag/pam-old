use super::logging::{DaemonLog, LogLevel, format_utc_for_test};

fn temp_dir(label: &str) -> std::path::PathBuf {
    let directory = std::env::temp_dir().join(format!(
        "pam-logging-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(&directory).expect("temp dir creates");
    directory
}

#[test]
fn ring_keeps_only_the_newest_entries_in_order() {
    let log = DaemonLog::open(&temp_dir("ring"));
    for index in 0..600 {
        log.info(format!("entry {index}"));
    }
    let recent = log.recent(2048);
    assert_eq!(recent.len(), 512);
    assert_eq!(recent.first().expect("first entry").message, "entry 88");
    assert_eq!(recent.last().expect("last entry").message, "entry 599");
    let tail = log.recent(3);
    assert_eq!(tail.len(), 3);
    assert_eq!(tail[0].message, "entry 597");
}

#[test]
fn messages_are_truncated_at_a_char_boundary() {
    let log = DaemonLog::open(&temp_dir("truncate"));
    let long = "é".repeat(2000);
    log.warn(&long);
    let recent = log.recent(1);
    let message = &recent[0].message;
    assert!(message.len() <= 1024);
    assert!(message.chars().all(|character| character == 'é'));
    assert_eq!(recent[0].level, LogLevel::Warn);
}

#[test]
fn file_appends_and_rotates_past_the_size_limit() {
    let directory = temp_dir("rotate");
    let log = DaemonLog::with_max_file_bytes(&directory, 200);
    for index in 0..20 {
        log.error(format!("failure number {index}"));
    }
    log.error("marker after rotation");
    let current = std::fs::read_to_string(directory.join("daemon.log")).expect("log file exists");
    let rotated =
        std::fs::read_to_string(directory.join("daemon.log.1")).expect("rotated file exists");
    assert!(current.contains("marker after rotation"));
    assert!(rotated.contains("ERROR"));
    assert!(current.len() < 400);
}

#[test]
fn missing_directory_still_serves_the_ring() {
    let log = DaemonLog::open(std::path::Path::new(
        "/nonexistent-root-for-pam-tests/logs",
    ));
    log.info("survives without a file");
    assert_eq!(log.recent(10).len(), 1);
}

#[test]
fn utc_formatting_matches_known_timestamps() {
    assert_eq!(format_utc_for_test(0), "1970-01-01T00:00:00.000Z");
    // 2026-08-28T00:00:00Z == 1_787_875_200 seconds.
    assert_eq!(
        format_utc_for_test(1_787_875_200_123),
        "2026-08-28T00:00:00.123Z"
    );
    // Leap-day check: 2024-02-29T12:00:00Z == 1_709_208_000 seconds.
    assert_eq!(
        format_utc_for_test(1_709_208_000_000),
        "2024-02-29T12:00:00.000Z"
    );
}
