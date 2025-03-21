use crate::{Terminal, config::ProxySubcommand};
use async_trait::async_trait;
use miette::Result;
use pingora::lb::selection::weighted::Weighted;
use pingora::{lb::selection::SelectionAlgorithm, prelude::*};
use rand::SeedableRng;
use rand::distr::{Bernoulli, Distribution};
use rand::rngs::SmallRng;
use std::cell::RefCell;
use std::sync::{Arc, Mutex};

const DEFAULT_CANARY_CHANCE: f64 = 0.4;

pub struct Proxy {
    _terminal: Terminal,
    args: ProxySubcommand,
}

impl Proxy {
    pub fn new(terminal: Terminal, args: ProxySubcommand) -> Self {
        Self {
            _terminal: terminal,
            args,
        }
    }

    pub fn dispatch(self) -> Result<()> {
        let mut server = Server::new(None).unwrap();
        server.bootstrap();

        let proxy = MultiProxy::new(&self.args);
        let mut proxy_service = http_proxy_service(&server.configuration, proxy);
        proxy_service.add_tcp("0.0.0.0:8000");
        server.add_service(proxy_service);
        server.run_forever();
    }
}

struct MultiProxy {
    balancer: Arc<LoadBalancer<Weighted<CanarySelector>>>,
    is_canary: Box<dyn Fn(String) -> bool + Send + Sync>,
}

struct CanarySelector {
    dist: Bernoulli,
    rng: Mutex<RefCell<SmallRng>>,
}

impl SelectionAlgorithm for CanarySelector {
    fn new() -> Self {
        Self {
            dist: Bernoulli::new(DEFAULT_CANARY_CHANCE).unwrap(),
            rng: Mutex::new(RefCell::new(SmallRng::from_rng(&mut rand::rng()))),
        }
    }

    fn next(&self, _key: &[u8]) -> u64 {
        let rng_guard = self.rng.lock().unwrap();
        // NOTE: Without the explicit RefCell::borrow_mut function references,
        // (i.e. if you use self.borrow_mut instead), you'll call the MutexGuard's
        // version of the function instead of the RefCell's version, as the MutexGuard
        // shadowed the same-named function in this scope.
        let mut borrowed = RefCell::borrow_mut(&rng_guard);
        if self.dist.sample(&mut borrowed) {
            1
        } else {
            0
        }
    }
}

impl MultiProxy {
    pub fn new(flags: &ProxySubcommand) -> Self {
        let canary = flags.canary().clone();
        let baseline = flags.baseline().clone();
        let canary_ip = canary.clone();
        let is_canary = Box::new(move |ip| ip == canary_ip);
        let upstreams = LoadBalancer::try_from_iter([baseline, canary]).unwrap();

        Self {
            balancer: Arc::new(upstreams),
            is_canary,
        }
    }
}

#[async_trait]
impl ProxyHttp for MultiProxy {
    type CTX = ();

    fn new_ctx(&self) {}

    async fn logging(&self, session: &mut Session, _e: Option<&Error>, _ctx: &mut Self::CTX) {
        if let Some(resp) = session.response_written() {
            let status = resp.status;
            println!("{status}");
        }
    }

    async fn upstream_peer(
        &self,
        _session: &mut Session,
        _ctx: &mut (),
    ) -> pingora::Result<Box<HttpPeer>> {
        let upstream = self
            .balancer
            .select(b"", 256) // hash doesn't matter for round robin
            .unwrap();

        let addr = upstream.addr.clone();
        if (self.is_canary)(format!("{addr}")) {
            println!("Proxying to canary at {addr}");
        } else {
            println!("Proxying to baseline at {addr}");
        }

        let peer = Box::new(HttpPeer::new(upstream, false, "".to_owned()));
        Ok(peer)
    }
}
