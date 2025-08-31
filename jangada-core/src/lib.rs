use std::{fmt, time::Duration};

use tracing::{instrument, trace};

use crate::{
    types::{
        Action, AppendEntries, AppendEntriesReply, ElectionTimeoutCtx, Event, RequestVote,
        RequestVoteReply, RpcPayload, State,
    },
    util::ToDisplay,
};

pub mod types;

mod util;
pub use util::{DefaultMachineRng, MachineRng};

/// The consensus machine of a single server.
///
/// The `I` type parameter represents a server ID.
pub struct Machine<I> {
    /// The ID of this server.
    id: I,
    /// The IDs of the other peers (does not include this server).
    peer_ids: Vec<I>,
    /// This server's state.
    state: State,
    current_term: u64,
    voted_for: Option<I>,
    // ===== state for inflight RPCs =====
    /// The current term when the append entries action was issued.
    append_log_saved_term: u64,
    /// The count of votes for the current election. Resets whenever a new
    /// election starts.
    election_state: ElectionState,
    // ===== other =====
    leader_heartbeat_interval: Duration,
    /// Buffer of actions produced in a single tick. Actions inserted here will
    /// be picked up after the current [`Self::tick`] call.
    actions: Vec<Action<I>>,
    /// Rng support.
    rng: Box<dyn MachineRng>,
}

#[derive(Debug)]
struct ElectionState {
    term: u64,
    votes: u32,
}

impl<I> Machine<I>
where
    I: Clone + PartialEq + fmt::Debug,
{
    pub fn new(
        id: I,
        peer_ids: Vec<I>,
        leader_heartbeat_interval: Duration,
        rng: Box<dyn MachineRng>,
    ) -> Machine<I> {
        Machine {
            id,
            peer_ids,
            state: State::Follower,
            current_term: 1,
            voted_for: None,
            append_log_saved_term: 0,
            election_state: ElectionState { term: 0, votes: 0 },
            leader_heartbeat_interval,
            actions: Vec::with_capacity(16),
            rng,
        }
    }

    pub fn id(&self) -> &I {
        &self.id
    }

    pub fn peers(&self) -> &[I] {
        &self.peer_ids
    }

    /// Will clear the actions buffer for the next [`Self::tick`] after the
    /// returned [`ActionsGuard`] gets dropped.
    #[must_use]
    pub fn actions(&mut self) -> ActionsGuard<'_, I> {
        ActionsGuard(self)
    }

    #[instrument(
        skip_all,
        fields(
            node = ?self.id,
            term = ?self.current_term,
            state = ?self.state,
        ),
    )]
    pub fn tick(&mut self, event: Event<I>) {
        assert!(self.actions.is_empty());
        match event {
            Event::Start => self.start(),
            Event::ElectionTimeout((), ctx) => self.election_timeout(ctx),
            Event::LeaderHeartbeatTick => self.leader_heartbeat_tick(),
            Event::RpcReply(_src, RpcPayload::RequestVoteReply(r)) => self.request_vote_reply(r),
            Event::RpcReply(_src, RpcPayload::AppendEntriesReply(r)) => {
                self.append_entries_reply(r)
            }
            // (think about the other peer)
            Event::RpcReply(src, RpcPayload::RequestVote(p)) => self.request_vote(src, p),
            Event::RpcReply(src, RpcPayload::AppendEntries(p)) => self.append_entries(src, p),
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
        self.current_term += 1; // <---- new term
        self.voted_for = Some(self.id.clone());

        self.election_state = ElectionState {
            term: self.current_term,
            votes: 1, // Start as one to already account for the self vote.
        };
        trace!(?self.election_state, "starting election for new term");

        let payload = RpcPayload::RequestVote(RequestVote {
            term: self.election_state.term,
            candidate_id: self.id.clone(),
        });

        for peer_id in &self.peer_ids {
            trace!(?peer_id, "sending RequestVote");
            self.actions
                .push(Action::Rpc(peer_id.clone(), payload.clone()));
        }

        // Start another election timer in case this one isn't fruitful.
        self.run_election_timer();
    }

    #[instrument(skip_all, fields(reply_term = reply.term, election_state = ?self.election_state))]
    fn request_vote_reply(&mut self, reply: RequestVoteReply) {
        if self.state != State::Candidate {
            trace!("non-candidate in vote reply, bailing");
            return;
        }

        if reply.term > self.election_state.term {
            trace!("term out of date in reply");
            self.become_follower(reply.term);
        } else if reply.term == self.election_state.term && reply.granted {
            self.election_state.votes += 1;
            if self.is_majority(self.election_state.votes) {
                trace!("won election with {} votes", self.election_state.votes);
                self.become_leader();
            }
        }
    }

    // think on another peer's pov (the one *receiving* the RPC)
    fn request_vote(&mut self, src: I, rpc: RequestVote<I>) {
        if self.state == State::Dead {
            return;
        }
        trace!(?self.voted_for, "received RequestVote");
        if rpc.term > self.current_term {
            trace!("stale current term");
            self.become_follower(rpc.term);
            // no return here! — fallthrough
        }

        let valid_vote =
            self.voted_for.is_none() || self.voted_for.as_ref() == Some(&rpc.candidate_id);
        let granted = if rpc.term == self.current_term && valid_vote {
            self.voted_for = Some(rpc.candidate_id);
            self.run_election_timer(); // Reset election timer.
            true
        } else {
            false
        };

        trace!("granted vote? {granted}");
        let reply = RpcPayload::RequestVoteReply(RequestVoteReply {
            term: self.current_term,
            granted,
        });
        self.actions.push(Action::Rpc(src, reply));
    }

    fn become_leader(&mut self) {
        self.state = State::Leader;
        trace!("became leader!");
        self.actions.push(Action::StartLeaderHeartbeatTicker(
            self.leader_heartbeat_interval,
        ));
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

        self.append_log_saved_term = self.current_term;
        let payload = RpcPayload::AppendEntries(AppendEntries {
            term: self.current_term,
            leader_id: self.id.clone(),
        });

        for peer_id in &self.peer_ids {
            trace!(?peer_id, "sending AppendEntries");
            self.actions
                .push(Action::Rpc(peer_id.clone(), payload.clone()));
        }
    }

    #[instrument(skip_all, fields(reply_term = reply.term, saved_term = self.append_log_saved_term))]
    fn append_entries_reply(&mut self, reply: AppendEntriesReply) {
        if reply.term > self.append_log_saved_term {
            trace!("term out of date in AppendEntries reply");
            self.become_follower(reply.term);
            return;
        }
        trace!("ok");
    }

    // think on another peer's pov (the one *receiving* the RPC)
    fn append_entries(&mut self, src: I, rpc: AppendEntries<I>) {
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
        let reply = RpcPayload::AppendEntriesReply(AppendEntriesReply {
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

    fn is_majority(&self, received: u32) -> bool {
        (received as usize) * 2 > self.peer_ids.len() + 1
    }
}

pub struct ActionsGuard<'m, I>(&'m mut Machine<I>);

impl<I> std::ops::Deref for ActionsGuard<'_, I> {
    type Target = [Action<I>];

    fn deref(&self) -> &Self::Target {
        &self.0.actions
    }
}

impl<I> Drop for ActionsGuard<'_, I> {
    fn drop(&mut self) {
        self.0.actions.clear();
    }
}
