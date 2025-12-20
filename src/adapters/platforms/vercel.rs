#![allow(dead_code)]
// TODO: This module will not be dead code once we allow the Vercel
// platform to be constructed.

use crate::{
    Shutdownable,
    adapters::{backend::PlatformConfig},
    subsystems::ShutdownResult,
};

use super::Platform;
use async_trait::async_trait;
use derive_getters::Getters;
use miette::Result;

// Placeholder: Eventually, we will implement a proper Vercel client.
// For now, we mock this as a unit type since we know it will be a required
// parameter later.
type VercelClient = ();

#[derive(Getters)]
pub struct Vercel {
    client: VercelClient,
}

impl Vercel {
    pub fn new(client: VercelClient) -> Self {
        Self {
            client,
        }
    }
}

#[async_trait]
impl Platform for Vercel {
    fn get_config(&self) -> PlatformConfig {
        todo!();
    }

    async fn deploy(&mut self) -> Result<(String, String)> {
        todo!();
    }

    async fn yank_canary(&mut self) -> Result<()> {
        todo!();
    }

    async fn delete_canary(&mut self) -> Result<()> {
        todo!();
    }

    async fn promote_rollout(&mut self) -> Result<()> {
        todo!();
    }
}

#[async_trait]
impl Shutdownable for Vercel {
    async fn shutdown(&mut self) -> ShutdownResult {
        todo!();
    }
}
