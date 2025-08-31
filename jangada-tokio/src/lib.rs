use std::{fmt, future::pending, pin::Pin, time::Duration};

use jangada_core::{
    Machine,
    types::{Action, ElectionTimeoutCtx, Event, RpcPayload},
};
use tokio::{
    select,
    sync::mpsc,
    time::{Instant, Interval, Sleep, interval, sleep_until},
};
use tracing::{info, warn};

pub struct TokioDriver<I> {
    machine: Machine<I>,
    requester: Box<dyn Requester<I>>,

    event_tx: mpsc::Sender<Event<I>>,
    event_rx: mpsc::Receiver<Event<I>>,

    election_timeout: Pin<Box<Sleep>>,
    election_timeout_ctx: Option<ElectionTimeoutCtx>,

    leader_heartbeat_ticker: Option<Pin<Box<Interval>>>,
}

pub trait Requester<I> {
    /// Performs a fire-and-forget request.
    ///
    /// Implementor must call the handler with the outcome of the call.
    fn fire(&self, handle: TokioDriverHandle<I>, dest: I, action: RpcPayload<I>);
}

#[derive(Clone)]
pub struct TokioDriverHandle<I>(mpsc::Sender<Event<I>>);

impl<I> TokioDriverHandle<I>
where
    I: Clone + PartialEq + fmt::Debug,
{
    pub async fn submit_response(&self, src: I, payload: RpcPayload<I>) {
        self.0.send(Event::RpcReply(src, payload)).await.unwrap();
    }
}

impl<I> TokioDriver<I>
where
    I: Clone + PartialEq + fmt::Debug,
{
    pub fn new(
        requester: Box<dyn Requester<I>>,
        machine: Machine<I>,
    ) -> (TokioDriver<I>, TokioDriverHandle<I>) {
        let (event_tx, event_rx) = mpsc::channel(machine.peers().len() * 32);

        let handle = TokioDriverHandle(event_tx.clone());
        let driver = TokioDriver {
            machine,
            requester,
            event_tx,
            event_rx,
            election_timeout: Box::pin(sleep_until(far_future())),
            election_timeout_ctx: None,
            leader_heartbeat_ticker: None,
        };
        (driver, handle)
    }

    pub async fn start(mut self) {
        self.event_tx.send(Event::Start).await.unwrap();

        loop {
            let election_timeout = Self::maybe_election_timeout(&mut self.election_timeout);
            let event_fut = self.event_rx.recv();
            let leader_heartbeat_fut =
                Self::maybe_tick_leader_heartbeat(&mut self.leader_heartbeat_ticker);

            select! {
                _ = leader_heartbeat_fut => self.handle_leader_heartbeat_tick().await,
                _ = election_timeout => self.handle_election_timeout().await,
                event = event_fut => self.handle_event(event.expect("must have event")).await,
            }
        }
    }

    async fn maybe_election_timeout(sleep: &mut Pin<Box<Sleep>>) {
        let sleep = sleep.as_mut();
        if sleep.is_elapsed() {
            pending::<()>().await;
        } else {
            sleep.await;
        }
    }

    async fn maybe_tick_leader_heartbeat(interval: &mut Option<Pin<Box<Interval>>>) {
        let Some(ticker) = interval else {
            pending().await
        };
        ticker.tick().await;
    }

    async fn handle_leader_heartbeat_tick(&mut self) {
        self.event_tx
            .send(Event::LeaderHeartbeatTick)
            .await
            .unwrap();
    }

    async fn handle_election_timeout(&mut self) {
        warn!("handling election timeout");
        let ctx = self
            .election_timeout_ctx
            .take()
            .expect("must have a previously-defined election timeout ctx");
        self.event_tx
            .send(Event::ElectionTimeout((), ctx))
            .await
            .unwrap();
    }

    async fn handle_event(&mut self, event: Event<I>) {
        warn!(?event, "got event, will TICK machine...");
        self.machine.tick(event);

        // XX: How can we get rid of this allocation?
        let actions = (*self.machine.actions()).to_vec();

        for action in actions {
            info!(?action, "handling action");
            self.handle_action(action).await;
        }
    }

    async fn handle_action(&mut self, action: Action<I>) {
        match action {
            Action::StartElectionTimeout(duration, election_timeout_ctx) => {
                let deadline = Instant::now().checked_add(duration).unwrap();
                self.election_timeout_ctx = Some(election_timeout_ctx);
                self.election_timeout.as_mut().reset(deadline);
            }
            Action::StartLeaderHeartbeatTicker(period) => {
                self.leader_heartbeat_ticker = Some(Box::pin(interval(period)));
            }
            Action::StopLeaderHeartbeatTicker => {
                self.leader_heartbeat_ticker = None;
            }
            Action::Rpc(dest, payload) => {
                let handle = TokioDriverHandle(self.event_tx.clone());
                self.requester.fire(handle, dest, payload);
            }
        }
    }
}

fn far_future() -> Instant {
    const ONE_YEAR_SECS: u64 = 60 * 60 * 24 * 365;

    Instant::now()
        .checked_add(Duration::from_secs(ONE_YEAR_SECS))
        .unwrap()
}
