use std::time::{Duration, Instant};

use pam_client::{ClientExchange, ExchangeError, StatusError, StatusExchange};
use pam_platform::LocalEndpoint;
use pam_protocol::RequestEnvelope;

/// Sends one request through [`pam_client::request_exchange`], classifying a
/// deadline expiry on stderr.
///
/// The result is returned unchanged — readiness probes legitimately treat
/// `DeadlineExceeded` as "retry", so this wrapper never panics and never
/// alters the outcome. It only prints one marker line describing whether the
/// process had CPU while the deadline ran out.
///
/// # Errors
///
/// Exactly the errors of [`pam_client::request_exchange`], unmodified.
pub async fn request_exchange(
    endpoint: &LocalEndpoint,
    request: &RequestEnvelope,
    wait: Duration,
) -> Result<ClientExchange, ExchangeError> {
    let wall_start = Instant::now();
    let cpu_start = process_cpu_time();
    let result = pam_client::request_exchange(endpoint, request, wait).await;
    if matches!(result, Err(ExchangeError::DeadlineExceeded)) {
        report_deadline(wall_start.elapsed(), cpu_delta(cpu_start));
    }
    result
}

/// Sends one status request through [`pam_client::request_status`],
/// classifying a deadline expiry on stderr.
///
/// See [`request_exchange`]; the wrapping behaviour is identical.
///
/// # Errors
///
/// Exactly the errors of [`pam_client::request_status`], unmodified.
pub async fn request_status(
    endpoint: &LocalEndpoint,
    request: &RequestEnvelope,
    wait: Duration,
) -> Result<StatusExchange, StatusError> {
    let wall_start = Instant::now();
    let cpu_start = process_cpu_time();
    let result = pam_client::request_status(endpoint, request, wait).await;
    if matches!(result, Err(StatusError::DeadlineExceeded)) {
        report_deadline(wall_start.elapsed(), cpu_delta(cpu_start));
    }
    result
}

fn report_deadline(wall: Duration, cpu: Option<Duration>) {
    if let Some(line) = classify_deadline(wall, cpu) {
        eprintln!("{line}");
    }
}

/// Classifies a missed exchange deadline by process CPU versus wall time.
///
/// A blocked IPC wait burns no CPU, so low CPU across a long wait cannot
/// distinguish starvation from a deadlock — the CI second pass settles that
/// (load is nondeterministic, a hang is deterministic); high CPU with a
/// missed deadline means the process was scheduled and still late, which no
/// retry will fix. The bias is deliberate: a false STARVED costs one retry;
/// a false ENGAGED costs a red build. With no CPU reading there is nothing to
/// classify, so no marker is produced at all.
pub(crate) fn classify_deadline(wall: Duration, cpu: Option<Duration>) -> Option<String> {
    let cpu = cpu?;
    let wall_secs = wall.as_secs_f64();
    let cpu_secs = cpu.as_secs_f64();
    if cpu < wall {
        Some(format!(
            "PAM-TIMEOUT-STARVED: exchange deadline after {wall_secs:.1}s with {cpu_secs:.2}s process CPU — consistent with runner starvation"
        ))
    } else {
        Some(format!(
            "PAM-TIMEOUT-ENGAGED: exchange deadline after {wall_secs:.1}s with {cpu_secs:.2}s process CPU — the process was running; treat as a defect"
        ))
    }
}

fn cpu_delta(start: Option<Duration>) -> Option<Duration> {
    let end = process_cpu_time()?;
    Some(end.saturating_sub(start?))
}

/// Total process CPU time (user plus system), or `None` when unreadable.
#[cfg(unix)]
pub(crate) fn process_cpu_time() -> Option<Duration> {
    use nix::sys::resource::{UsageWho, getrusage};

    let usage = getrusage(UsageWho::RUSAGE_SELF).ok()?;
    let user = timeval_duration(usage.user_time())?;
    let system = timeval_duration(usage.system_time())?;
    user.checked_add(system)
}

#[cfg(unix)]
fn timeval_duration(value: nix::sys::time::TimeVal) -> Option<Duration> {
    let secs = u64::try_from(value.tv_sec()).ok()?;
    let micros = u64::try_from(value.tv_usec()).ok()?;
    Duration::from_secs(secs).checked_add(Duration::from_micros(micros))
}

/// Total process CPU time is unreadable off unix; deadline expiries there get
/// no marker rather than a guessed classification.
#[cfg(not(unix))]
pub(crate) fn process_cpu_time() -> Option<Duration> {
    None
}
