//! Non-macOS sandbox stub.
//!
//! The MVP only implements copy-on-write sandboxing on macOS (APFS). On other
//! platforms this stub keeps the crate building but fails fast with a clear
//! diagnostic. Linux/Windows CoW support is tracked under *Future work*.

use std::path::Path;

use async_trait::async_trait;
use miette::{Result, miette};

use super::{Sandbox, SandboxHandle};

/// A sandbox that is not available on this platform.
pub struct UnsupportedSandbox;

#[async_trait]
impl Sandbox for UnsupportedSandbox {
    async fn create(&self, _source: &Path) -> Result<SandboxHandle> {
        Err(miette!(
            "copy-on-write sandboxing is only implemented on macOS in this MVP; this platform is not yet supported"
        ))
    }
}
