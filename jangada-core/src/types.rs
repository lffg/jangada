use std::time::Duration;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
#[derive(Debug)]
pub enum Event<I> {
    Start,

    /// By [`Action::StartElectionTimeout`].
    ElectionTimeout((), ElectionTimeoutCtx),

    /// By [`Action::StartLeaderHeartbeatTicker`].
    LeaderHeartbeatTick,

    /// *Source* node ID and the reply payload.
    RpcReply(I, RpcPayload<I>),
}

/// An *output* action which indicates that some action is to be carried out.
///
/// All variants are tuple-like with two elements. The first one contains the
/// actual action payload; the second contains additional context to be copied
/// to the event that this action may yield.
#[derive(Debug, Clone)]
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
    Rpc(I, RpcPayload<I>),
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(
    feature = "serde",
    serde(rename_all = "SCREAMING_SNAKE_CASE", tag = "type")
)]
pub enum RpcPayload<I> {
    RequestVote(RequestVote<I>),
    RequestVoteReply(RequestVoteReply),
    AppendEntries(AppendEntries<I>),
    AppendEntriesReply(AppendEntriesReply),
}

#[derive(Debug, Clone)]
pub struct ElectionTimeoutCtx {
    pub term_started: u64,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RequestVote<I> {
    pub term: u64,
    pub candidate_id: I,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RequestVoteReply {
    pub term: u64,
    pub granted: bool,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AppendEntries<I> {
    pub term: u64,
    pub leader_id: I,
    // TODO: prev_log_index: u64, prev_log_term: u64, entries: Vec<LogEntry>, leader_commit: u64,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AppendEntriesReply {
    pub term: u64,
    pub success: bool,
}
