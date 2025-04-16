use failsafe::backoff::Exponential;
use failsafe::failure_policy::ConsecutiveFailures;
use failsafe::futures::CircuitBreaker as AsyncCircuitBreaker;
use failsafe::{Config, Error, backoff, failure_policy};
use miette::{Result, bail};
use multitool_sdk::apis::Error as SDKError;
use tokio::time::Duration;
use tracing::debug;

pub struct CircuitBreaker {
    config: Config<ConsecutiveFailures<Exponential>, ()>,
}

impl CircuitBreaker {
    pub fn new() -> Self {
        let backoff = backoff::exponential(Duration::from_secs(1), Duration::from_secs(4));
        let policy = failure_policy::consecutive_failures(3, backoff);
        let config = Config::new().failure_policy(policy);
        Self { config }
    }

    pub async fn call<T, F: AsyncFn() -> Result<T>>(self, func: F) -> Result<T> {
        let circuit_breaker = self.config.build();

        let check_err = |err: &SDKError<_>| matches!(err, SDKError::ResponseError(e) if [503, 429, 504, 522].contains(&e.status.as_u16()) );

        loop {
            let mut last_err = None;

            match circuit_breaker.call_with(check_err, func).await {
                Err(Error::Inner(e)) => {
                    debug!("Got an error, retrying...");
                    last_err = Some(e);
                }
                Err(Error::Rejected) => {
                    debug!("Got too many errors, giving up...");
                    bail!(last_err.unwrap());
                }
                Ok(res) => return res,
            }
        }
    }
}
