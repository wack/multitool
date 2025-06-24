#![cfg(feature = "gateway")]

use crate::Terminal;
use futures_util::StreamExt;
use gateway_crds::GatewayClass;
use k8s_openapi::api::core::v1::Pod;
use kube::{
    Api, Client, ResourceExt,
    runtime::controller::{Action, Controller},
};
use miette::{IntoDiagnostic as _, Report, miette};
use std::future::ready;
use std::{sync::Arc, time::Duration};
use tokio::runtime::Runtime;
use tracing::info;

pub struct Gateway {
    _terminal: Terminal,
}

#[derive(thiserror::Error, Debug)]
pub enum Error {}
pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Gateway {
    pub fn new(terminal: Terminal) -> Self {
        Self {
            _terminal: terminal,
        }
    }

    pub fn dispatch(self) -> miette::Result<()> {
        let rt = Runtime::new().into_diagnostic()?;
        let _guard = rt.enter();
        rt.block_on(async {
            info!("Starting the MultiTool API Gateway!");
            self.run_gateway().await.map_err(Report::msg)
        })
    }

    async fn run_gateway(self) -> miette::Result<()> {
        // Create a new Kubernetes client using the credentials
        // stored natively in the cluster.
        info!("Creating client.");
        let client = Client::try_default()
            .await
            .map_err(|err| miette!("Cannot create Kubernetes client: {err:?}"))?;

        info!("Watching pods.");
        // Watch for changes to GatewayClass resources.
        let gateway_classes = Api::<GatewayClass>::all(client);

        info!("Starting controller");
        Controller::new(gateway_classes.clone(), Default::default())
            .run(reconcile, error_policy, Arc::new(()))
            .for_each(|_| ready(()))
            .await;

        Ok(())
    }
}

async fn reconcile(obj: Arc<GatewayClass>, ctx: Arc<()>) -> Result<Action> {
    info!("reconcile request: {}", obj.name_any());
    Ok(Action::requeue(Duration::from_secs(3600)))
}

fn error_policy(_object: Arc<GatewayClass>, _err: &Error, _ctx: Arc<()>) -> Action {
    Action::requeue(Duration::from_secs(5))
}
