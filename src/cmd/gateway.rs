#![cfg(feature = "gateway")]

use crate::Terminal;
use crate::config::GatewayMode;
use crate::config::GatewaySubcommand;
use futures_util::StreamExt;
use gateway_crds::{Gateway as GatewayResource, GatewayClass};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{Condition, Time};
use k8s_openapi::chrono::Utc;
use kube::{
    Api, Client, ResourceExt,
    api::{Patch, PatchParams},
    runtime::{
        controller::{Action, Controller},
        watcher,
    },
};
use miette::{IntoDiagnostic as _, Report, miette};
use serde_json::json;
use std::future::ready;
use std::{sync::Arc, time::Duration};
use tokio::{runtime::Runtime, select};
use tracing::info;

pub struct Gateway {
    _terminal: Terminal,
    flags: GatewaySubcommand,
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Kubernetes error: {0}")]
    Kube(#[from] kube::Error),
}
pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Gateway {
    pub fn new(terminal: Terminal, flags: GatewaySubcommand) -> Self {
        Self {
            _terminal: terminal,
            flags,
        }
    }

    pub fn dispatch(self) -> miette::Result<()> {
        let rt = Runtime::new().into_diagnostic()?;
        let _guard = rt.enter();
        rt.block_on(async {
            info!("Starting the MultiTool API Gateway!");
            match self.flags.mode() {
                GatewayMode::ApiServer => self.run_api_server().await,
                GatewayMode::Controller => self.run_controller().await.map_err(Report::msg),
            }
        })
    }

    async fn run_api_server(self) -> miette::Result<()> {
        todo!();
    }

    async fn run_controller(self) -> miette::Result<()> {
        // Create a new Kubernetes client using the credentials
        // stored natively in the cluster.
        info!("Creating client.");
        let client = Client::try_default()
            .await
            .map_err(|err| miette!("Cannot create Kubernetes client: {err:?}"))?;

        info!("Watching GatewayClass and Gateway CRDs.");
        // Watch for changes to GatewayClass resources.
        let gateway_classes = Api::<GatewayClass>::all(client.clone());
        // Watch for changes to Gateway resources.
        let gateways = Api::<GatewayResource>::all(client);

        info!("Starting controllers");
        let gateway_class_controller = Controller::new(gateway_classes.clone(), watcher::Config::default())
            .run(reconcile_gateway_class, error_policy_gateway_class, Arc::new(()))
            .for_each(|_| ready(()));

        let gateway_controller = Controller::new(gateways, watcher::Config::default())
            .run(reconcile_gateway, error_policy_gateway, Arc::new(()))
            .for_each(|_| ready(()));

        select! {
            _ = gateway_class_controller => {},
            _ = gateway_controller => {},
        }

        Ok(())
    }
}

async fn update_gateway_class_status(
    gateway_class: &GatewayClass,
) -> Result<GatewayClass, kube::Error> {
    // Create a Kubernetes client to update the status
    let client = Client::try_default().await?;
    let api: Api<GatewayClass> = Api::all(client);

    let now = Time(Utc::now());
    let condition = Condition {
        type_: "Accepted".to_string(),
        status: "True".to_string(),
        observed_generation: gateway_class.metadata.generation,
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
        &gateway_class.name_any(),
        &PatchParams::default(),
        &Patch::Merge(&status),
    )
    .await
}

async fn update_gateway_status(
    gateway: &GatewayResource,
) -> Result<GatewayResource, kube::Error> {
    // Create a Kubernetes client to update the status
    let client = Client::try_default().await?;
    let api: Api<GatewayResource> = Api::namespaced(client, gateway.namespace().as_deref().unwrap_or("default"));

    let now = Time(Utc::now());
    let condition = Condition {
        type_: "Accepted".to_string(),
        status: "True".to_string(),
        observed_generation: gateway.metadata.generation,
        last_transition_time: now,
        reason: "Accepted".to_string(),
        message: "Gateway accepted by controller".to_string(),
    };

    let status = json!({
        "status": {
            "conditions": [condition]
        }
    });

    api.patch_status(
        &gateway.name_any(),
        &PatchParams::default(),
        &Patch::Merge(&status),
    )
    .await
}

async fn reconcile_gateway_class(obj: Arc<GatewayClass>, _ctx: Arc<()>) -> Result<Action> {
    info!("reconcile request: {}", obj.name_any());

    if obj.spec.controller_name != "multitool.run/multitool" {
        return Ok(Action::requeue(Duration::from_secs(3600)));
    }

    // Check if status is already set to Accepted
    if !is_accepted(&*obj) {
        update_gateway_class_status(&*obj).await?;
        info!("Updated GatewayClass {} status to Accepted", obj.name_any());
    }

    Ok(Action::requeue(Duration::from_secs(3600)))
}

// Check if a gateway class has been accepted.
fn is_accepted(gateway_class: &GatewayClass) -> bool {
    gateway_class
        .status
        .as_ref()
        .and_then(|status| status.conditions.as_ref())
        .map(|conditions| {
            conditions
                .iter()
                .any(|condition| condition.type_ == "Accepted" && condition.status == "True")
        })
        .unwrap_or(false)
}

// Check if a gateway has been accepted.
fn is_gateway_accepted(gateway: &GatewayResource) -> bool {
    gateway
        .status
        .as_ref()
        .and_then(|status| status.conditions.as_ref())
        .map(|conditions| {
            conditions
                .iter()
                .any(|condition| condition.type_ == "Accepted" && condition.status == "True")
        })
        .unwrap_or(false)
}

fn error_policy_gateway_class(_object: Arc<GatewayClass>, _err: &Error, _ctx: Arc<()>) -> Action {
    Action::requeue(Duration::from_secs(5))
}

fn error_policy_gateway(_object: Arc<GatewayResource>, _err: &Error, _ctx: Arc<()>) -> Action {
    Action::requeue(Duration::from_secs(5))
}


async fn reconcile_gateway(obj: Arc<GatewayResource>, _ctx: Arc<()>) -> Result<Action> {
    info!("Gateway reconcile request: {}", obj.name_any());

    // Check if the Gateway references a GatewayClass that our controller manages
    // For now, we'll accept all Gateways - this could be filtered later
    
    // Check if status is already set to Accepted
    if !is_gateway_accepted(&*obj) {
        update_gateway_status(&*obj).await?;
        info!("Updated Gateway {} status to Accepted", obj.name_any());
    }

    Ok(Action::requeue(Duration::from_secs(3600)))
}
