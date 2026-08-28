#![forbid(unsafe_code)]

mod error;
mod status;
#[cfg(test)]
mod status_test;

pub use error::{ExchangeError, StatusError};
pub use status::{
    ClientExchange, StatusExchange, StreamingExchange, StreamingExchangeError, request_exchange,
    request_exchange_streaming, request_status,
};
