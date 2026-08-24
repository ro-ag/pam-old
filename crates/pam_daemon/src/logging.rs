//! Bounded daemon diagnostics: every entry lands in an in-memory ring buffer
//! (served to the debug console) and, best effort, in a size-rotated log file
//! next to the durable state so a crash leaves a trail even when the daemon
//! runs detached with discarded stdio.

use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// Entries kept for the debug console.
const RING_CAPACITY: usize = 512;
/// Longest recorded message; longer input is truncated at a char boundary.
const MAX_MESSAGE_BYTES: usize = 1024;
/// Rotation threshold for the on-disk log.
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

impl LogLevel {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
        }
    }
}

#[derive(Clone, Debug)]
pub struct LogEntry {
    pub timestamp_ms: u64,
    pub level: LogLevel,
    pub message: String,
}

struct Inner {
    ring: VecDeque<LogEntry>,
    file: Option<File>,
    path: PathBuf,
    file_bytes: u64,
    max_file_bytes: u64,
}

/// Cloneable logging handle shared by the serve loop, the scheduler, and
/// request handlers. All operations are best effort and never fail.
#[derive(Clone)]
pub struct DaemonLog(Arc<Mutex<Inner>>);

impl DaemonLog {
    /// Opens (or creates) `daemon.log` inside `directory`. File problems are
    /// swallowed: the ring buffer keeps working without a file.
    #[must_use]
    pub fn open(directory: &Path) -> Self {
        Self::with_max_file_bytes(directory, MAX_FILE_BYTES)
    }

    pub(crate) fn with_max_file_bytes(directory: &Path, max_file_bytes: u64) -> Self {
        let path = directory.join("daemon.log");
        let file = fs::create_dir_all(directory).ok().and_then(|()| {
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .ok()
        });
        let file_bytes = file
            .as_ref()
            .and_then(|file| file.metadata().ok())
            .map_or(0, |metadata| metadata.len());
        Self(Arc::new(Mutex::new(Inner {
            ring: VecDeque::with_capacity(RING_CAPACITY),
            file,
            path,
            file_bytes,
            max_file_bytes,
        })))
    }

    pub fn info(&self, message: impl AsRef<str>) {
        self.record(LogLevel::Info, message.as_ref());
    }

    pub fn warn(&self, message: impl AsRef<str>) {
        self.record(LogLevel::Warn, message.as_ref());
    }

    pub fn error(&self, message: impl AsRef<str>) {
        self.record(LogLevel::Error, message.as_ref());
    }

    fn record(&self, level: LogLevel, message: &str) {
        let entry = LogEntry {
            timestamp_ms: now_ms(),
            level,
            message: truncate_at_char_boundary(message, MAX_MESSAGE_BYTES).to_owned(),
        };
        let Ok(mut inner) = self.0.lock() else {
            return;
        };
        if inner.ring.len() == RING_CAPACITY {
            inner.ring.pop_front();
        }
        inner.ring.push_back(entry.clone());
        inner.append_line(&entry);
    }

    /// Most recent entries, oldest first, at most `limit`.
    #[must_use]
    pub fn recent(&self, limit: usize) -> Vec<LogEntry> {
        let Ok(inner) = self.0.lock() else {
            return Vec::new();
        };
        let skip = inner.ring.len().saturating_sub(limit);
        inner.ring.iter().skip(skip).cloned().collect()
    }
}

impl Inner {
    fn append_line(&mut self, entry: &LogEntry) {
        let Some(file) = self.file.as_mut() else {
            return;
        };
        let line = format!(
            "{} {} {}\n",
            format_utc(entry.timestamp_ms),
            entry.level.as_str(),
            entry.message
        );
        if file.write_all(line.as_bytes()).is_err() {
            return;
        }
        self.file_bytes = self.file_bytes.saturating_add(line.len() as u64);
        if self.file_bytes > self.max_file_bytes {
            self.rotate();
        }
    }

    fn rotate(&mut self) {
        self.file = None;
        let rotated = self.path.with_extension("log.1");
        let _ = fs::rename(&self.path, rotated);
        self.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .ok();
        self.file_bytes = 0;
    }
}

fn truncate_at_char_boundary(message: &str, max_bytes: usize) -> &str {
    if message.len() <= max_bytes {
        return message;
    }
    let mut end = max_bytes;
    while end > 0 && !message.is_char_boundary(end) {
        end -= 1;
    }
    &message[..end]
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

/// Formats epoch milliseconds as `YYYY-MM-DDTHH:MM:SS.mmmZ` without a
/// calendar dependency (civil-from-days algorithm).
fn format_utc(timestamp_ms: u64) -> String {
    let seconds = timestamp_ms / 1000;
    let millis = timestamp_ms % 1000;
    let days = i64::try_from(seconds / 86_400).unwrap_or(0);
    let second_of_day = seconds % 86_400;
    let (hour, minute, second) = (
        second_of_day / 3600,
        (second_of_day % 3600) / 60,
        second_of_day % 60,
    );
    let era_day = days + 719_468;
    let era = era_day.div_euclid(146_097);
    let day_of_era = era_day.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

#[cfg(test)]
pub(crate) fn format_utc_for_test(timestamp_ms: u64) -> String {
    format_utc(timestamp_ms)
}
