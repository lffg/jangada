use std::{cell::Cell, rc::Rc, time::Duration};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum State {
    Follower,
    Candidate,
    Leader,
    Dead,
}

/// An *input* event that drives the [`Machine`] forward.
///
/// All variants are tuple-like with two elements. The first one contain the
/// actual event payload; the second contains additional context copied directly
/// from the action that triggered this event.
pub enum Event<I> {
    Start,

    /// By [`Action::StartElectionTimeout`].
    ElectionTimeout((), ElectionTimeoutCtx),

    /// By [`Action::StartLeaderHeartbeatTicker`].
    LeaderHeartbeatTick,

    /// *Source* node ID and the reply payload.
    RpcReply(I, RpcEvent<I>),
}

pub enum RpcEvent<I> {
    /// The incoming RPC.
    RequestVote(RequestVote<I>),

    /// The incoming reply of [`RpcAction::RequestVote`].
    RequestVoteReply(RequestVoteReply, RequestVoteCtx),

    /// The incoming RPC.
    AppendEntries(AppendEntries<I>),

    /// The incoming reply of [`RpcAction::AppendEntries`].
    AppendEntriesReply(AppendEntriesReply, AppendEntriesCtx),
}

/// An *output* action which indicates that some action is to be carried out.
///
/// All variants are tuple-like with two elements. The first one contains the
/// actual action payload; the second contains additional context to be copied
/// to the event that this action may yield.
pub enum Action<I> {
    /// Will trigger one [`Event::ElectionTimeout`].
    ///
    /// **CONTRACT:** The machine may yield multiple actions for this variant.
    /// Whenever another concurrent call is issued, the previous one **MUST** be
    /// cancelled in favor of the newer one.
    StartElectionTimeout(Duration, ElectionTimeoutCtx),

    /// Will trigger events of type [`Event::LeaderHeartbeatTick`] continuously
    /// until [`Action::StopLeaderHeartbeatTicker`] is issued.
    StartLeaderHeartbeatTicker(Duration),

    /// Stops the ticker started by [`Action::StartLeaderHeartbeatTicker`].
    StopLeaderHeartbeatTicker,

    /// *Destination* node ID and the action payload.
    Rpc(I, RpcAction<I>),
}

pub enum RpcAction<I> {
    /// Will trigger zero or more [`RpcEvent::RequestVoteReply`]. (More than one
    /// since we don't guarantee exactly-once delivery.)
    RequestVote(RequestVote<I>, RequestVoteCtx),
    RequestVoteReply(RequestVoteReply),

    /// Will trigger zero or more [`RpcEvent::AppendEntriesReply`].
    AppendEntries(AppendEntries<I>, AppendEntriesCtx),
    AppendEntriesReply(AppendEntriesReply),
}

pub struct ElectionTimeoutCtx {
    pub term_started: u64,
}

#[derive(Clone)]
pub struct RequestVote<I> {
    pub term: u64,
    pub candidate_id: I,
}

#[derive(Clone)]
pub struct RequestVoteCtx {
    pub election_term: u64,
    pub votes_received: Rc<Cell<usize>>,
}

pub struct RequestVoteReply {
    pub term: u64,
    pub granted: bool,
}

#[derive(Clone)]
pub struct AppendEntries<I> {
    pub term: u64,
    pub leader_id: I,
    // TODO: prev_log_index: u64, prev_log_term: u64, entries: Vec<LogEntry>, leader_commit: u64,
}

#[derive(Clone)]
pub struct AppendEntriesCtx {
    /// The current term when the append entries action was issued.
    pub saved_term: u64,
}

pub struct AppendEntriesReply {
    pub term: u64,
    pub success: bool,
}
