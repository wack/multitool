#![cfg(feature = "gateway")]

use crate::Terminal;
use futures_util::StreamExt;
use gateway_crds::GatewayClass;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{Condition, Time};
use k8s_openapi::chrono::Utc;
use kube::{
    Api, Client, ResourceExt,
    api::{Patch, PatchParams},
    runtime::controller::{Action, Controller},
};
use miette::{IntoDiagnostic as _, Report, miette};
use serde_json::json;
use std::future::ready;
use std::{sync::Arc, time::Duration};
use tokio::runtime::Runtime;
use tracing::info;

pub struct Gateway {
    _terminal: Terminal,
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Kubernetes error: {0}")]
    Kube(#[from] kube::Error),
}
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

    if obj.spec.controller_name != "multitool.run/multitool" {
        return Ok(Action::requeue(Duration::from_secs(3600)));
    }

    // Check if status is already set to Accepted
    let is_accepted = obj
        .status
        .as_ref()
        .and_then(|status| status.conditions.as_ref())
        .map(|conditions| {
            conditions
                .iter()
                .any(|condition| condition.type_ == "Accepted" && condition.status == "True")
        })
        .unwrap_or(false);

    if !is_accepted {
        // Create a Kubernetes client to update the status
        let client = Client::try_default().await?;
        let api: Api<GatewayClass> = Api::all(client);

        let now = Time(Utc::now());
        let condition = Condition {
            type_: "Accepted".to_string(),
            status: "True".to_string(),
            observed_generation: obj.metadata.generation,
            last_transition_time: now,
            reason: "Accepted".to_string(),
            message: "GatewayClass accepted by controller".to_string(),
        };

        let status = json!({
            "status": {
                "conditions": [condition]
            }
        });

        api.patch_status(
            &obj.name_any(),
            &PatchParams::default(),
            &Patch::Merge(&status),
        )
        .await?;

        info!("Updated GatewayClass {} status to Accepted", obj.name_any());
    }

    Ok(Action::requeue(Duration::from_secs(3600)))
}

fn error_policy(_object: Arc<GatewayClass>, _err: &Error, _ctx: Arc<()>) -> Action {
    Action::requeue(Duration::from_secs(5))
}
