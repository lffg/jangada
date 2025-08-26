use std::{cell::Cell, rc::Rc, time::Duration};

use tracing::{instrument, trace};

use crate::{
    types::{
        Action, AppendEntries, AppendEntriesCtx, AppendEntriesReply, ElectionTimeoutCtx, Event,
        RequestVote, RequestVoteCtx, RequestVoteReply, RpcAction, RpcEvent, State,
    },
    util::ToDisplay,
};

pub mod types;

mod util;
pub use util::{DefaultMachineRng, MachineRng};

/// The consensus machine of a single server.
pub struct Machine {
    /// The ID of this server.
    id: u64,
    /// The ID of the other peers (does not include this server).
    peer_ids: Vec<u64>,
    /// This server's state.
    state: State,
    current_term: u64,
    voted_for: Option<u64>,
    // ===== other =====
    /// Buffer of actions produced in a single tick. Actions inserted here will
    /// be picked up after the current [`Self::tick`] call.
    actions: Vec<Action>,
    /// Rng support.
    rng: Box<dyn MachineRng>,
}

impl Machine {
    pub fn new(id: u64, peer_ids: Vec<u64>, rng: Box<dyn MachineRng>) -> Machine {
        Machine {
            id,
            peer_ids,
            state: State::Follower,
            current_term: 1,
            voted_for: None,
            actions: Vec::with_capacity(16),
            rng,
        }
    }

    /// Will clear the actions buffer for the next [`Self::tick`] after the
    /// returned [`ActionsGuard`] gets dropped.
    #[must_use]
    pub fn actions(&mut self) -> ActionsGuard<'_> {
        ActionsGuard(self)
    }

    #[instrument(
        skip_all,
        fields(
            node = %self.id,
            term = ?self.current_term,
            state = ?self.state,
        ),
    )]
    pub fn tick(&mut self, event: Event) {
        assert!(self.actions.is_empty());
        match event {
            Event::Start => self.start(),
            Event::ElectionTimeout((), ctx) => self.election_timeout(ctx),
            Event::LeaderHeartbeatTick => self.leader_heartbeat_tick(),
            Event::RpcReply(_src, RpcEvent::RequestVoteReply(r, ctx)) => {
                self.request_vote_reply(r, ctx)
            }
            Event::RpcReply(_src, RpcEvent::AppendEntriesReply(r, ctx)) => {
                self.append_entries_reply(r, ctx)
            }
            // (think about the other peer)
            Event::RpcReply(src, RpcEvent::RequestVote(p)) => self.request_vote(src, p),
            Event::RpcReply(src, RpcEvent::AppendEntries(p)) => self.append_entries(src, p),
        }
    }

    fn start(&mut self) {
        self.run_election_timer();
    }

    fn run_election_timer(&mut self) {
        let duration = self.rng.next_election_timeout();
        trace!("scheduled election timer ({})", duration.display());
        self.actions.push({
            let ctx = ElectionTimeoutCtx {
                term_started: self.current_term,
            };
            Action::StartElectionTimeout(duration, ctx)
        });
    }

    fn election_timeout(&mut self, ctx: ElectionTimeoutCtx) {
        let term_started = ctx.term_started;

        if self.state == State::Leader {
            trace!("leader in election timeout, bailing");
            return;
        }
        if self.current_term != term_started {
            trace!(
                term_started,
                "changed term while elapsing election timeout, bailing",
            );
            return;
        }
        self.start_election();
    }

    fn start_election(&mut self) {
        self.state = State::Candidate;
        self.current_term += 1;
        self.voted_for = Some(self.id);

        let election_term = self.current_term;
        trace!(election_term, "starting election for new term");

        let payload = RequestVote {
            term: election_term,
            candidate_id: self.id,
        };
        let ctx = RequestVoteCtx {
            votes_received: Rc::new(Cell::new(1)),
            election_term,
        };

        for peer_id in &self.peer_ids {
            trace!(peer_id, "sending RequestVote");
            let rpc = RpcAction::RequestVote(payload.clone(), ctx.clone());
            self.actions.push(Action::Rpc(*peer_id, rpc));
        }

        // Start another election timer in case this one isn't fruitful.
        self.run_election_timer();
    }

    #[instrument(skip_all, fields(reply_term = reply.term, election_term = ctx.election_term))]
    fn request_vote_reply(&mut self, reply: RequestVoteReply, ctx: RequestVoteCtx) {
        if self.state != State::Candidate {
            trace!("non-candidate in vote reply, bailing");
            return;
        }

        if reply.term > ctx.election_term {
            trace!("term out of date in reply");
            self.become_follower(reply.term);
        } else if reply.term == ctx.election_term && reply.granted {
            ctx.votes_received.update(|v| v + 1);
            let votes = ctx.votes_received.get();
            if self.is_majority(votes) {
                trace!("won election with {votes} votes");
                self.become_leader();
            }
        }
    }

    // think on another peer's pov (the one *receiving* the RPC)
    fn request_vote(&mut self, src: u64, rpc: RequestVote) {
        if self.state == State::Dead {
            return;
        }
        trace!(?self.voted_for, "received RequestVote");
        if rpc.term > self.current_term {
            trace!("stale current term");
            self.become_follower(rpc.term);
            // no return here! — fallthrough
        }

        let valid_vote = self.voted_for.is_some_and(|v| v == rpc.candidate_id);
        let granted = if rpc.term == self.current_term && valid_vote {
            self.voted_for = Some(rpc.candidate_id);
            self.run_election_timer(); // Reset election timer.
            true
        } else {
            false
        };

        trace!("granted vote? {granted}");
        let reply = RpcAction::RequestVoteReply(RequestVoteReply {
            term: self.current_term,
            granted,
        });
        self.actions.push(Action::Rpc(src, reply));
    }

    fn become_leader(&mut self) {
        self.state = State::Leader;
        trace!("became leader!");

        let duration = Duration::from_millis(50);
        self.actions
            .push(Action::StartLeaderHeartbeatTicker(duration));
    }

    fn leader_heartbeat_tick(&mut self) {
        if self.state != State::Leader {
            self.actions.push(Action::StopLeaderHeartbeatTicker);
            return;
        }
        self.leader_heartbeat_send();
    }

    fn leader_heartbeat_send(&mut self) {
        assert_eq!(self.state, State::Leader);

        let payload = AppendEntries {
            term: self.current_term,
            leader_id: self.id,
        };
        let ctx = AppendEntriesCtx {
            saved_term: self.current_term,
        };

        for peer_id in &self.peer_ids {
            trace!(peer_id, "sending AppendEntries");
            let rpc = RpcAction::AppendEntries(payload.clone(), ctx.clone());
            self.actions.push(Action::Rpc(*peer_id, rpc));
        }
    }

    #[instrument(skip_all, fields(reply_term = reply.term, saved_term = ctx.saved_term))]
    fn append_entries_reply(&mut self, reply: AppendEntriesReply, ctx: AppendEntriesCtx) {
        if reply.term > ctx.saved_term {
            trace!("term out of date in AppendEntries reply");
            self.become_follower(reply.term);
            return;
        }
        trace!("ok");
    }

    // think on another peer's pov (the one *receiving* the RPC)
    fn append_entries(&mut self, src: u64, rpc: AppendEntries) {
        if self.state == State::Dead {
            return;
        }
        trace!(?self.voted_for, "received AppendEntries");
        if rpc.term > self.current_term {
            trace!("stale current term");
            self.become_follower(rpc.term);
            // no return here! — fallthrough
        }

        let mut success = false;
        if rpc.term == self.current_term {
            if self.state != State::Follower {
                self.become_follower(rpc.term);
            }
            self.run_election_timer(); // Reset election timer.
            success = true;
        }

        trace!("success? {success}");
        let reply = RpcAction::AppendEntriesReply(AppendEntriesReply {
            term: self.current_term,
            success,
        });
        self.actions.push(Action::Rpc(src, reply));
    }

    fn become_follower(&mut self, new_term: u64) {
        self.state = State::Follower;
        self.current_term = new_term;
        self.voted_for = None;

        trace!("became follower, new term is {new_term}");
        self.run_election_timer();
    }

    fn is_majority(&self, received: usize) -> bool {
        received * 2 > self.peer_ids.len() + 1
    }
}

pub struct ActionsGuard<'m>(&'m mut Machine);

impl ActionsGuard<'_> {
    pub fn actions(&self) -> &[Action] {
        &self.0.actions
    }
}

impl Drop for ActionsGuard<'_> {
    fn drop(&mut self) {
        self.0.actions.clear();
    }
}
