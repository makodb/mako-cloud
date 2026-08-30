use std::{collections::VecDeque, error::Error, fmt, num::NonZeroUsize};

use mako_documents::{
    ChangeLogError, CheckpointStatus, DocumentReadAuthorizer, ReadAuthorizationPath,
    RetentionError, SchemaVersion, ScopedCollectionEngine,
};
use mako_identity::AccessAuthorizationEpochs;

use crate::{
    AuthenticatedReplicationContext, LiveStreamEvent, LiveStreamRequest, OpaqueStreamCursor,
    ReplicationContractError, ReplicationFilter, ReplicationTokenCodec, ReplicationTokenCodecError,
    ResyncReason,
    schema::{SchemaMigrationRequired, require_compatible_schema},
    visibility::replication_change,
};

pub const MAX_LIVE_EVENT_BATCH_SIZE: usize = 1_000;
pub const MAX_LIVE_BUFFER_EVENTS: usize = 1_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LiveStreamLimits {
    event_batch_size: NonZeroUsize,
    buffer_capacity: NonZeroUsize,
}

impl LiveStreamLimits {
    #[must_use]
    pub const fn new(event_batch_size: NonZeroUsize, buffer_capacity: NonZeroUsize) -> Self {
        Self {
            event_batch_size,
            buffer_capacity,
        }
    }
}

pub struct LiveStreamSession<'a> {
    collection: &'a ScopedCollectionEngine,
    tokens: ReplicationTokenCodec<'a>,
    context: AuthenticatedReplicationContext,
    authorizer: &'a dyn DocumentReadAuthorizer,
    schema_version: u64,
    event_batch_size: NonZeroUsize,
    buffer_capacity: NonZeroUsize,
    last_scanned_position: u64,
    buffer: VecDeque<LiveStreamEvent>,
    requires_resync: bool,
    filter: Option<ReplicationFilter>,
}

impl<'a> LiveStreamSession<'a> {
    pub async fn open(
        collection: &'a ScopedCollectionEngine,
        tokens: ReplicationTokenCodec<'a>,
        context: AuthenticatedReplicationContext,
        authorizer: &'a dyn DocumentReadAuthorizer,
        required_schema_version: SchemaVersion,
        request: &LiveStreamRequest,
        limits: LiveStreamLimits,
    ) -> Result<Self, LiveStreamError> {
        request.validate()?;
        require_compatible_schema(required_schema_version, request.schema_version, &context)?;
        if limits.event_batch_size.get() > MAX_LIVE_EVENT_BATCH_SIZE
            || limits.buffer_capacity.get() > MAX_LIVE_BUFFER_EVENTS
        {
            return Err(LiveStreamError::InvalidLimits);
        }
        if collection.scope().tenant() != context.tenant()
            || collection.scope().collection_id() != context.collection_id()
        {
            return Err(LiveStreamError::ScopeMismatch);
        }
        let committed_high_water = collection.capture_committed_high_water().await?;
        let (last_scanned_position, initial_resync) = if let Some(cursor) = &request.cursor {
            let cursor = tokens.decode_stream_cursor(
                cursor,
                &context,
                request.schema_version,
                request.filter.as_ref(),
            )?;
            (cursor.after_position(), Some(ResyncReason::Reconnected))
        } else if let Some(checkpoint) = &request.checkpoint {
            let position = tokens
                .decode_checkpoint(
                    checkpoint,
                    &context,
                    request.schema_version,
                    request.filter.as_ref(),
                )?
                .scanned_position();
            let reason = match collection.checkpoint_status(position).await? {
                CheckpointStatus::Valid => None,
                CheckpointStatus::Expired { .. } => Some(ResyncReason::CheckpointExpired),
            };
            (position, reason)
        } else {
            (committed_high_water, None)
        };
        if last_scanned_position > committed_high_water {
            return Err(LiveStreamError::PositionBeyondHighWater);
        }
        let mut session = Self {
            collection,
            tokens,
            context,
            authorizer,
            schema_version: request.schema_version,
            event_batch_size: limits.event_batch_size,
            buffer_capacity: limits.buffer_capacity,
            last_scanned_position,
            buffer: VecDeque::new(),
            requires_resync: initial_resync.is_some(),
            filter: request.filter.clone(),
        };
        if let Some(reason) = initial_resync {
            session.force_resync(reason);
        }
        Ok(session)
    }

    pub async fn poll(&mut self) -> Result<(), LiveStreamError> {
        if self.requires_resync {
            return Ok(());
        }
        if matches!(
            self.collection
                .checkpoint_status(self.last_scanned_position)
                .await?,
            CheckpointStatus::Expired { .. }
        ) {
            self.force_resync(ResyncReason::CheckpointExpired);
            return Ok(());
        }
        let high_water = self.collection.capture_committed_high_water().await?;
        if self.last_scanned_position >= high_water {
            return Ok(());
        }
        let page = self
            .collection
            .read_change_page(
                self.last_scanned_position,
                high_water,
                self.event_batch_size,
            )
            .await?;
        let mut documents = Vec::new();
        for change in page.changes() {
            self.last_scanned_position = change.change().commit_position();
            if let Some(document) = replication_change(
                self.collection,
                ReadAuthorizationPath::LiveStream,
                self.authorizer,
                change,
                self.filter.as_ref(),
            ) {
                documents.push(document);
            }
        }
        if page.is_exhausted() {
            self.last_scanned_position = high_water;
        }
        if page.changes().is_empty() && !page.is_exhausted() {
            return Err(LiveStreamError::ChangeScanDidNotAdvance);
        }
        let checkpoint = self.tokens.encode_checkpoint(
            &self.context,
            self.schema_version,
            self.last_scanned_position,
            None,
            self.filter.as_ref(),
        )?;
        let cursor = self.cursor(high_water)?;
        if documents.is_empty() {
            self.enqueue(LiveStreamEvent::Checkpoint { checkpoint, cursor });
        } else {
            self.enqueue(LiveStreamEvent::Documents {
                documents,
                checkpoint,
                cursor,
            });
        }
        Ok(())
    }

    pub async fn heartbeat(&mut self) -> Result<(), LiveStreamError> {
        if self.requires_resync {
            return Ok(());
        }
        let high_water = self.collection.capture_committed_high_water().await?;
        self.enqueue(LiveStreamEvent::Heartbeat {
            cursor: self.cursor(high_water)?,
        });
        Ok(())
    }

    #[must_use]
    pub fn requires_resync(&self) -> bool {
        self.requires_resync
    }

    #[must_use]
    pub const fn last_scanned_position(&self) -> u64 {
        self.last_scanned_position
    }

    pub fn observe_authorization_epochs(&mut self, current: AccessAuthorizationEpochs) {
        if current != self.context.authorization_epochs() {
            self.force_resync(ResyncReason::AuthorizationEpochChanged);
        }
    }

    pub fn signal_service_failover(&mut self) {
        self.force_resync(ResyncReason::ServiceFailover);
    }

    pub fn signal_stream_gap(&mut self) {
        self.force_resync(ResyncReason::StreamGap);
    }

    pub fn drain_events(&mut self) -> impl Iterator<Item = LiveStreamEvent> + '_ {
        self.buffer.drain(..)
    }

    fn cursor(&self, high_water: u64) -> Result<OpaqueStreamCursor, LiveStreamError> {
        Ok(self.tokens.encode_stream_cursor(
            &self.context,
            self.schema_version,
            self.last_scanned_position,
            high_water.max(self.last_scanned_position),
            None,
            self.filter.as_ref(),
        )?)
    }

    fn enqueue(&mut self, event: LiveStreamEvent) {
        if self.buffer.len() >= self.buffer_capacity.get() {
            self.force_resync(ResyncReason::StreamGap);
            return;
        }
        self.buffer.push_back(event);
    }

    fn force_resync(&mut self, reason: ResyncReason) {
        self.buffer.clear();
        self.buffer.push_back(LiveStreamEvent::Resync { reason });
        self.requires_resync = true;
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SseFrame(String);

impl SseFrame {
    pub fn from_event(event: &LiveStreamEvent) -> Result<Self, LiveStreamError> {
        let (name, id) = match event {
            LiveStreamEvent::Documents { cursor, .. } => ("documents", Some(cursor.as_str())),
            LiveStreamEvent::Checkpoint { cursor, .. } => ("checkpoint", Some(cursor.as_str())),
            LiveStreamEvent::Heartbeat { cursor } => ("heartbeat", Some(cursor.as_str())),
            LiveStreamEvent::Resync { .. } => ("resync", None),
        };
        let data = serde_json::to_string(event)?;
        let id = id.map_or_else(String::new, |id| format!("id: {id}\n"));
        Ok(Self(format!("event: {name}\n{id}data: {data}\n\n")))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug)]
pub enum LiveStreamError {
    Contract(ReplicationContractError),
    Token(ReplicationTokenCodecError),
    ChangeLog(ChangeLogError),
    Retention(RetentionError),
    Json(serde_json::Error),
    SchemaMigrationRequired(SchemaMigrationRequired),
    InvalidLimits,
    ScopeMismatch,
    PositionBeyondHighWater,
    ChangeScanDidNotAdvance,
}

impl fmt::Display for LiveStreamError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Contract(error) => error.fmt(formatter),
            Self::Token(error) => error.fmt(formatter),
            Self::ChangeLog(error) => error.fmt(formatter),
            Self::Retention(error) => error.fmt(formatter),
            Self::Json(error) => error.fmt(formatter),
            Self::SchemaMigrationRequired(error) => error.fmt(formatter),
            Self::InvalidLimits => formatter.write_str("live-stream limits are invalid"),
            Self::ScopeMismatch => formatter.write_str("replication collection scope mismatch"),
            Self::PositionBeyondHighWater => {
                formatter.write_str("live-stream position exceeds committed high water")
            }
            Self::ChangeScanDidNotAdvance => {
                formatter.write_str("live-stream change scan did not advance")
            }
        }
    }
}

impl Error for LiveStreamError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Contract(error) => Some(error),
            Self::Token(error) => Some(error),
            Self::ChangeLog(error) => Some(error),
            Self::Retention(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::SchemaMigrationRequired(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ReplicationContractError> for LiveStreamError {
    fn from(error: ReplicationContractError) -> Self {
        Self::Contract(error)
    }
}

impl From<ReplicationTokenCodecError> for LiveStreamError {
    fn from(error: ReplicationTokenCodecError) -> Self {
        Self::Token(error)
    }
}

impl From<ChangeLogError> for LiveStreamError {
    fn from(error: ChangeLogError) -> Self {
        Self::ChangeLog(error)
    }
}

impl From<RetentionError> for LiveStreamError {
    fn from(error: RetentionError) -> Self {
        Self::Retention(error)
    }
}

impl From<serde_json::Error> for LiveStreamError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<SchemaMigrationRequired> for LiveStreamError {
    fn from(error: SchemaMigrationRequired) -> Self {
        Self::SchemaMigrationRequired(error)
    }
}

#[cfg(test)]
mod tests {
    use std::{num::NonZeroU64, sync::Arc};

    use mako_api::{CollectionId, CollectionScope, EnvironmentId, ProjectId, TenantScope};
    use mako_documents::{
        CollectionLifecycle, CollectionMetadata, CollectionMetadataVersion, CommitPosition,
        DocumentEngine, DocumentValidator, MutationAuthorizationDecision, MutationId,
        MutationInput, PrimaryKeyDefinition, ReadAuthorizationContext, SchemaCompatibility,
        SchemaVersion,
    };
    use mako_identity::{AccessAuthorizationEpochs, AppUserId, SessionId};
    use mako_storage::{Durability, MemoryAdapter};
    use serde_json::json;

    use super::*;
    use crate::ReplicationTokenKey;

    struct Visibility(bool);

    impl DocumentReadAuthorizer for Visibility {
        fn authorize_read(&self, _: ReadAuthorizationContext<'_>) -> MutationAuthorizationDecision {
            if self.0 {
                MutationAuthorizationDecision::allow("test_allow")
            } else {
                MutationAuthorizationDecision::deny("test_deny")
            }
        }
    }

    #[test]
    fn live_delivery_filters_frames_heartbeats_and_overflow_resyncs() {
        futures::executor::block_on(async {
            let tenant = tenant();
            let engine = DocumentEngine::new(Arc::new(MemoryAdapter::new()));
            let collection = engine
                .scope_collection(
                    &tenant,
                    CollectionScope::new(
                        tenant.clone(),
                        CollectionId::parse("todos").expect("collection"),
                    ),
                )
                .expect("collection");
            let sequencer = engine
                .scope_sequencer(&tenant, &tenant, Durability::Memory)
                .expect("sequencer");
            let validator = validator();
            commit(&collection, &sequencer, &validator, "todo-a", 1).await;
            let context = context();
            let key = ReplicationTokenKey::from_bytes([5; 32]);
            let checkpoint = ReplicationTokenCodec::new(&key)
                .encode_checkpoint(&context, 1, 0, None, None)
                .expect("checkpoint");
            let request = LiveStreamRequest {
                schema_version: 1,
                checkpoint: Some(checkpoint.clone()),
                cursor: None,
                filter: None,
            };
            let visible = Visibility(true);
            assert!(matches!(
                LiveStreamSession::open(
                    &collection,
                    ReplicationTokenCodec::new(&key),
                    context.clone(),
                    &visible,
                    SchemaVersion::new(2).expect("schema version"),
                    &request,
                    limits(10, 2),
                )
                .await,
                Err(LiveStreamError::SchemaMigrationRequired(_))
            ));
            assert!(matches!(
                LiveStreamSession::open(
                    &collection,
                    ReplicationTokenCodec::new(&key),
                    context.clone(),
                    &visible,
                    SchemaVersion::new(1).expect("schema version"),
                    &request,
                    limits(MAX_LIVE_EVENT_BATCH_SIZE + 1, 2),
                )
                .await,
                Err(LiveStreamError::InvalidLimits)
            ));
            let mut session = LiveStreamSession::open(
                &collection,
                ReplicationTokenCodec::new(&key),
                context.clone(),
                &visible,
                SchemaVersion::new(1).expect("schema version"),
                &request,
                limits(10, 2),
            )
            .await
            .expect("open");
            session.poll().await.expect("poll");
            session.heartbeat().await.expect("heartbeat");
            let events = session.drain_events().collect::<Vec<_>>();
            assert_eq!(events.len(), 2);
            let LiveStreamEvent::Documents {
                documents, cursor, ..
            } = &events[0]
            else {
                panic!("expected documents");
            };
            assert_eq!(documents[0]["id"], "todo-a");
            let frame = SseFrame::from_event(&events[0]).expect("frame");
            assert!(frame.as_str().starts_with("event: documents\nid: msc1."));
            assert!(frame.as_str().contains("\ndata: "));

            let hidden = Visibility(false);
            let mut hidden_session = LiveStreamSession::open(
                &collection,
                ReplicationTokenCodec::new(&key),
                context.clone(),
                &hidden,
                SchemaVersion::new(1).expect("schema version"),
                &request,
                limits(10, 2),
            )
            .await
            .expect("open hidden");
            hidden_session.poll().await.expect("hidden poll");
            assert!(matches!(
                hidden_session.drain_events().next(),
                Some(LiveStreamEvent::Checkpoint { .. })
            ));

            let mut slow = LiveStreamSession::open(
                &collection,
                ReplicationTokenCodec::new(&key),
                context.clone(),
                &visible,
                SchemaVersion::new(1).expect("schema version"),
                &request,
                limits(1, 1),
            )
            .await
            .expect("slow");
            slow.poll().await.expect("first poll");
            commit(&collection, &sequencer, &validator, "todo-b", 2).await;
            slow.poll().await.expect("overflow poll");
            assert!(slow.requires_resync());
            assert!(matches!(
                slow.drain_events().next(),
                Some(LiveStreamEvent::Resync {
                    reason: ResyncReason::StreamGap
                })
            ));

            let reconnect_request = LiveStreamRequest {
                schema_version: 1,
                checkpoint: None,
                cursor: Some(cursor.clone()),
                filter: None,
            };
            let mut reconnect = LiveStreamSession::open(
                &collection,
                ReplicationTokenCodec::new(&key),
                context.clone(),
                &visible,
                SchemaVersion::new(1).expect("schema version"),
                &reconnect_request,
                limits(1, 1),
            )
            .await
            .expect("reconnect");
            assert!(reconnect.requires_resync());
            assert!(matches!(
                reconnect.drain_events().next(),
                Some(LiveStreamEvent::Resync {
                    reason: ResyncReason::Reconnected
                })
            ));

            let fresh_request = LiveStreamRequest {
                schema_version: 1,
                checkpoint: None,
                cursor: None,
                filter: None,
            };
            let mut signaled = LiveStreamSession::open(
                &collection,
                ReplicationTokenCodec::new(&key),
                context.clone(),
                &visible,
                SchemaVersion::new(1).expect("schema version"),
                &fresh_request,
                limits(1, 1),
            )
            .await
            .expect("signal session");
            signaled.observe_authorization_epochs(AccessAuthorizationEpochs {
                environment: 2,
                user: 1,
            });
            assert!(matches!(
                signaled.drain_events().next(),
                Some(LiveStreamEvent::Resync {
                    reason: ResyncReason::AuthorizationEpochChanged
                })
            ));
            signaled.signal_service_failover();
            assert!(matches!(
                signaled.drain_events().next(),
                Some(LiveStreamEvent::Resync {
                    reason: ResyncReason::ServiceFailover
                })
            ));
            signaled.signal_stream_gap();
            assert!(matches!(
                signaled.drain_events().next(),
                Some(LiveStreamEvent::Resync {
                    reason: ResyncReason::StreamGap
                })
            ));

            let checkpoint_at_one = ReplicationTokenCodec::new(&key)
                .encode_checkpoint(&context, 1, 1, None, None)
                .expect("checkpoint at one");
            let mut expiring = LiveStreamSession::open(
                &collection,
                ReplicationTokenCodec::new(&key),
                context.clone(),
                &visible,
                SchemaVersion::new(1).expect("schema version"),
                &LiveStreamRequest {
                    schema_version: 1,
                    checkpoint: Some(checkpoint_at_one),
                    cursor: None,
                    filter: None,
                },
                limits(1, 1),
            )
            .await
            .expect("expiring session");
            collection
                .compact_through(2, Durability::Memory)
                .await
                .expect("compact");
            expiring.poll().await.expect("expired checkpoint poll");
            assert!(matches!(
                expiring.drain_events().next(),
                Some(LiveStreamEvent::Resync {
                    reason: ResyncReason::CheckpointExpired
                })
            ));

            let mut already_expired = LiveStreamSession::open(
                &collection,
                ReplicationTokenCodec::new(&key),
                context,
                &visible,
                SchemaVersion::new(1).expect("schema version"),
                &request,
                limits(1, 1),
            )
            .await
            .expect("already expired session");
            assert!(matches!(
                already_expired.drain_events().next(),
                Some(LiveStreamEvent::Resync {
                    reason: ResyncReason::CheckpointExpired
                })
            ));
        });
    }

    async fn commit(
        collection: &ScopedCollectionEngine,
        sequencer: &mako_documents::EnvironmentSequencer,
        validator: &DocumentValidator,
        id: &str,
        value: u64,
    ) {
        let mut lease = sequencer
            .lease(NonZeroU64::new(1).expect("lease"))
            .await
            .expect("lease");
        let position = lease.issue().expect("position");
        collection
            .create_document(MutationInput {
                mutation_id: MutationId::parse(format!("mutation-{id}")).expect("mutation"),
                commit_position: CommitPosition::new(position).expect("position"),
                document: validator
                    .validate_create(json!({"id": id, "value": value}))
                    .expect("document"),
                durability: Durability::Memory,
            })
            .await
            .expect("commit");
        sequencer.recover_high_water().await.expect("high water");
    }

    fn limits(event_batch_size: usize, buffer_capacity: usize) -> LiveStreamLimits {
        LiveStreamLimits::new(
            NonZeroUsize::new(event_batch_size).expect("batch"),
            NonZeroUsize::new(buffer_capacity).expect("buffer"),
        )
    }

    fn validator() -> DocumentValidator {
        let metadata = CollectionMetadata::new(
            CollectionId::parse("todos").expect("collection"),
            CollectionMetadataVersion::new(1).expect("metadata version"),
            SchemaVersion::new(1).expect("schema version"),
            json!({
                "type": "object",
                "required": ["id", "value"],
                "properties": {
                    "id": {"type": "string"},
                    "value": {"type": "integer"}
                },
                "additionalProperties": false
            }),
            PrimaryKeyDefinition::field("id").expect("primary key"),
            SchemaCompatibility::Compatible,
            CollectionLifecycle::Active,
        )
        .expect("metadata");
        DocumentValidator::compile(&metadata).expect("validator")
    }

    fn context() -> AuthenticatedReplicationContext {
        AuthenticatedReplicationContext::new(
            tenant(),
            CollectionId::parse("todos").expect("collection"),
            AppUserId::parse("usr_abcdefgh").expect("user"),
            SessionId::parse("ses_abcdefgh").expect("session"),
            "member",
            AccessAuthorizationEpochs {
                environment: 1,
                user: 1,
            },
            "req_abcdefgh",
        )
        .expect("context")
    }

    fn tenant() -> TenantScope {
        TenantScope::new(
            ProjectId::parse("prj_abcdefgh").expect("project"),
            EnvironmentId::parse("env_abcdefgh").expect("environment"),
        )
    }
}
