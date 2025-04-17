//! The `CircuitBreaker` is a type that can replay a retriable future,
//! and classify errors as fatal or retriable. After certain failure requirements
//1 are met, the circuit "breaks" and no more retries are made, returning an error.
//! Circuit breakers are excellent for handling transient errors like network
//! packet drops or time-based errors.
//!

use std::num::NonZeroUsize;

use bon::bon;
use failsafe::backoff::Exponential;
use failsafe::failure_policy::ConsecutiveFailures;
use failsafe::futures::CircuitBreaker as AsyncCircuitBreaker;
use failsafe::{Config, Error, backoff, failure_policy};
use futures_core::TryFuture;
use miette::{Result, bail};
use multitool_sdk::apis::Error as SDKError;
use tokio::time::Duration;
use tracing::debug;

use super::ManyError;

/// An `HttpCircuitBreaker` is an implementation of the `CircuitBreaker`
/// pattern specialized for HTTP requests. HTTP requests typically fail
/// due to network hiccups and timeouts, so this circuit breaker retries
/// with exponential backoff since these errors are typically transient.
pub struct HttpCircuitBreaker
// <T, F>
// where
//     F: AsyncFn() -> Result<T>,
{
    config: Config<ConsecutiveFailures<Exponential>, ()>,
    // A retriable future.
    //    fut: F,
}

#[bon]
impl HttpCircuitBreaker {
    /// This is the number of retries we attempt before breaking the circuit.
    const DEFAULT_HTTP_RETRIES: usize = 4;
    /// This is the default starting point for exponential backoff. The first request
    /// will wait this long before retrying.
    const DEFAULT_BACKOFF_START: Duration = Duration::from_secs(1);
    /// This is the maximum backoff duration. The start value increases
    /// exponentially each iteration until it reaches this value. It is
    /// not clear what the exponential constant is.
    const DEFAULT_BACKOFF_END: Duration = Duration::from_secs(5);

    #[builder]
    fn new(
        retries: Option<NonZeroUsize>,
        backoff_start: Option<Duration>,
        backoff_end: Option<Duration>,
    ) -> Self {
        // Coalsence the optional arguments with the default values.
        let backoff_start = backoff_start.unwrap_or(Self::DEFAULT_BACKOFF_START);
        let backoff_end = backoff_end.unwrap_or(Self::DEFAULT_BACKOFF_END);
        let retry_count: u32 = retries
            .map(NonZeroUsize::get)
            .unwrap_or(Self::DEFAULT_HTTP_RETRIES) as u32;
        // Create a backoff policy using the values.
        let backoff = backoff::exponential(backoff_start, backoff_end);
        let retry_policy = failure_policy::consecutive_failures(retry_count, backoff);
        let config = Config::new().failure_policy(retry_policy);
        Self { config }
    }

    async fn call<T, F>(self, func: F) -> Result<T>
    where
        F: AsyncFn() -> Result<T>,
    {
        let circuit_breaker = self.config.build();
        // let check_err = |err: &SDKError<_>| matches!(err, SDKError::ResponseError(e) if [503, 429, 504, 522].contains(&e.status.as_u16()) );
        // let check_err = |err: | false;

        // Now, we try the future repeatedly until either it succeeds
        // or the circuit breaks.
        // If it breaks, we return the list of errors we received.
        loop {
            let mut err = ManyError::default();
            // Create the next future, passing it into the breaker
            // to await.
            let next_fut = func();
            let exec_result = circuit_breaker.call(next_fut);
            // Inspect the error, if any, and capture it before
            // retrying.
            match exec_result.await {
                Ok(ok) => return Ok(ok),
                Err(Error::Inner(e)) => {
                    debug!("Got an error, retrying...");
                    err.append(e);
                }
                Err(Error::Rejected) => {
                    debug!("Got too many errors, giving up...");
                    bail!(err);
                }
            }
        }
    }
}

// impl CircuitBreaker {
//     pub fn new() -> Self {
//         let backoff = backoff::exponential(Duration::from_secs(1), Duration::from_secs(4));
//         let policy = failure_policy::consecutive_failures(3, backoff);
//         let config = Config::new().failure_policy(policy);
//         Self { config }
//     }

//     pub async fn call<T, F: AsyncFn() -> Result<T>>(self, func: F) -> Result<T> {
//         let circuit_breaker = self.config.build();

//         let check_err = |err: &SDKError<_>| matches!(err, SDKError::ResponseError(e) if [503, 429, 504, 522].contains(&e.status.as_u16()) );

//         loop {
//             let mut last_err = None;

//             match circuit_breaker.call_with(check_err, func).await {
//                 Err(Error::Inner(e)) => {
//                     debug!("Got an error, retrying...");
//                     last_err = Some(e);
//                 }
//                 Err(Error::Rejected) => {
//                     debug!("Got too many errors, giving up...");
//                     bail!(last_err.unwrap());
//                 }
//                 Ok(res) => return res,
//             }
//         }
//     }
// }
