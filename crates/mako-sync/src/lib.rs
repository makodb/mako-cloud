//! RxDB pull, push, checkpoint, and live-stream service.

#![forbid(unsafe_code)]

mod codec;
mod contract;
mod live;
mod pull;
mod push;
mod schema;
mod visibility;

pub use codec::{
    ReplicationTokenCodec, ReplicationTokenCodecError, ReplicationTokenKey, VerifiedCheckpoint,
    VerifiedStreamCursor,
};
pub use contract::{
    AuthenticatedReplicationContext, LiveStreamEvent, LiveStreamRequest, MAX_MUTATION_ID_BYTES,
    MAX_PULL_BATCH_SIZE, MAX_PUSH_BATCH_SIZE, OpaqueCheckpoint, OpaqueStreamCursor, PullRequest,
    PullResponse, PushOutcome, PushOutcomeStatus, PushRequest, PushResponse, PushRow,
    ReplicationContractError, ResyncReason,
};
pub use live::{
    LiveStreamError, LiveStreamLimits, LiveStreamSession, MAX_LIVE_BUFFER_EVENTS,
    MAX_LIVE_EVENT_BATCH_SIZE, SseFrame,
};
pub use pull::{PullError, PullService};
pub use push::{PushError, PushService};
pub use schema::SchemaMigrationRequired;

/// Identifies this workspace component in diagnostics.
pub const COMPONENT: &str = "sync";
