use std::{net::SocketAddr, sync::Arc};

use axum::{Json, Router, extract::State, routing::post};
use clap::Parser;
use jangada_core::{Machine, types::RpcPayload};
use jangada_tokio::{Requester, TokioDriver, TokioDriverHandle};
use serde::{Deserialize, Serialize};
use tracing::{info, trace, warn};

#[derive(Parser)]
struct Opts {
    #[arg(long)]
    id: SocketAddr,
    #[arg(short = 'p', long)]
    peers: Vec<SocketAddr>,
    #[arg(long, default_value_t = 300)]
    constant: u64,
    #[arg(long, default_value_t = 3)]
    factor: u64,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let opts = Arc::new(Opts::parse());

    let rng = Box::new(jangada_core::DefaultMachineRng::with_params(
        opts.constant,
        opts.factor,
    ));
    let machine = Machine::new(opts.id, opts.peers.clone(), rng);

    let requester = Box::new(HttpRequester::new(opts.id));
    let (driver, handle) = TokioDriver::new(requester, machine);

    tokio::spawn({
        let opts = Arc::clone(&opts);
        async move {
            http_server(&opts, handle).await;
        }
    });

    warn!("will start");
    driver.start().await;
}

async fn http_server(opts: &Opts, handle: TokioDriverHandle<SocketAddr>) {
    let app = Router::new()
        .route("/raft/action", post(handle_raft_action))
        .with_state(handle);

    let listener = tokio::net::TcpListener::bind(opts.id).await.unwrap();
    let addr = listener.local_addr().unwrap();
    info!("HTTP server listening at {addr}");

    axum::serve(listener, app).await.unwrap();
}

#[derive(Serialize, Deserialize)]
struct RequestPayload {
    id: SocketAddr,
    payload: RpcPayload<SocketAddr>,
}

async fn handle_raft_action(
    State(handle): State<TokioDriverHandle<SocketAddr>>,
    Json(RequestPayload { id: src, payload }): Json<RequestPayload>,
) {
    trace!("got rpc from {src}: {payload:?}");
    handle.submit_response(src, payload).await;
}

struct HttpRequester(Arc<HttpRequesterCtx>);

impl HttpRequester {
    pub fn new(id: SocketAddr) -> HttpRequester {
        HttpRequester(Arc::new(HttpRequesterCtx {
            id,
            client: reqwest::Client::new(),
        }))
    }
}

struct HttpRequesterCtx {
    id: SocketAddr,
    client: reqwest::Client,
}

impl Requester<SocketAddr> for HttpRequester {
    fn fire(
        &self,
        _handle: TokioDriverHandle<SocketAddr>,
        dest: SocketAddr,
        payload: RpcPayload<SocketAddr>,
    ) {
        let inner = Arc::clone(&self.0);
        trace!("sending http request to server {dest}");
        tokio::spawn(async move {
            let result = inner
                .client
                .post(format!("http://{dest}/raft/action")) // XX: HTTPS?
                .json(&RequestPayload {
                    id: inner.id,
                    payload,
                })
                .send()
                .await;
            if let Err(error) = result {
                warn!(?error, "request failed");
            }
        });
    }
}
