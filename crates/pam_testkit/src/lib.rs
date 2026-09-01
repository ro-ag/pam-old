//! Test-only client helpers that classify exchange deadline expiries.
//!
//! Wraps [`pam_client`] request helpers so a `DeadlineExceeded` result also
//! prints one stderr marker saying whether the process was starved of CPU
//! (retry the run) or was running and still missed the deadline (a defect).

#![forbid(unsafe_code)]

mod patience;
#[cfg(test)]
mod patience_test;

pub use patience::{request_exchange, request_status};
