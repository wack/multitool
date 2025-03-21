use aws_config::{BehaviorVersion, SdkConfig};
use tokio::sync::OnceCell;

/// Load AWS configuration using their standard rules. e.g. AWS_ACCESS_KEY_ID,
/// or session profile information, etc. This function fetches the data only
/// once, the first time it's called, and memoized the results, so all future
/// calls with return the same information. This prevents a race condition where
/// an external process changes an environment variable while this process is running.
pub async fn load_default_aws_config() -> &'static SdkConfig {
    AWS_CONFIG_CELL.get_or_init(load_config).await
}

/// Private, delegate function to be called only within a OnceCell to ensure
/// its locked. When Rust supports async closures, we can move this into a closure
/// to guarantee its only ever called in one place.
async fn load_config() -> SdkConfig {
    // We don't need a particular version, but we pin to one to ensure
    // it doesn't accidently slip if `latest` gets updated without our knowledge.
    let behavior = BehaviorVersion::v2025_01_17();
    aws_config::load_defaults(behavior).await
}

static AWS_CONFIG_CELL: OnceCell<SdkConfig> = OnceCell::const_new();
