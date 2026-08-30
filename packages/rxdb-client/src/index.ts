/** RxDB replication and project-auth client. */

export const PACKAGE_NAME = "@mako-cloud/rxdb" as const;
export const PACKAGE_VERSION = "0.1.0" as const;

export {
  API_ERROR_VERSION,
  ERROR_CODES,
  ScopeValidationError,
  isApiErrorEnvelope,
  type ApiError,
  type CollectionId,
  type EnvironmentId,
  type ErrorCode,
  type MakoAuthSession,
  type MakoAuthUser,
  type MakoMagicLinkAccepted,
  type MakoProviderSignInStart,
  type MakoSignUpAccepted,
  type MakoStorageObjectPage,
  type ProjectId,
  type RetryAdvice,
  type SafeDetail,
  type SafeDetails,
  type ScopeValidationCode,
} from "./wire.js";

export {
  MAKO_RXDB_CONNECT_TEMPLATE_VERSION,
  createMakoRxdbConnectTemplateV1,
  type MakoRxdbConnectTemplateV1Input,
} from "./connect-template.js";

export {
  MakoAuthClient,
  MakoAuthError,
  MakoAuthenticationRequiredError,
  MemoryAuthSessionPersistence,
  type AuthSessionPersistence,
  type AuthUser,
  type MakoAuthClientOptions,
  type MakoAuthErrorOptions,
  type MakoAuthEvent,
  type MakoAuthEventKind,
  type MakoAuthEventListener,
  type MakoAuthUnsubscribe,
  type MakoSignInFragment,
  type MakoUserSession,
  type PersistedAuthSession,
  type ProviderSignInStart,
  type SignUpAccepted,
} from "./auth.js";

export {
  BrowserAuthSessionPersistence,
  type BrowserAuthSessionPersistenceOptions,
  type BrowserAuthSessionPersistenceScope,
} from "./browser-session.js";

export {
  MakoStorageClient,
  MakoStorageError,
  encodeObjectPath,
  type MakoStorageBody,
  type MakoStorageClientOptions,
  type MakoStorageListOptions,
  type MakoStorageListResult,
  type MakoStorageObject,
  type MakoStorageObjectRecord,
  type MakoStoragePutOptions,
  type MakoStoragePutResult,
  type MakoStorageScope,
} from "./storage.js";

export {
  MakoRxdbConfigurationError,
  SUPPORTED_RXDB_MAJOR,
  SUPPORTED_RXDB_RANGE,
  UnsupportedRxdbVersionError,
  assertSupportedRxdbVersion,
  normalizeMakoRxdbConfig,
  type MakoReplicationFilter,
  type MakoRxdbClientConfig,
  type MakoRxdbRuntime,
  type NormalizedMakoRxdbClientConfig,
} from "./config.js";

export {
  MakoLiveStreamGroup,
  createMakoLiveStreamGroup,
} from "./live-group.js";

export {
  createMakoPullHandler,
  createMakoPullOptions,
  type MakoCheckpoint,
  type MakoPullAdapterOptions,
} from "./pull.js";

export {
  MakoLivePullStream,
  createMakoLivePullStream,
  type MakoLiveStreamOptions,
  type MakoResyncReason,
} from "./live.js";

export {
  MakoAuthorizationEpochCoordinator,
  MemoryReplicationSecurityStatePersistence,
  type AuthorizationEpochCoordinatorOptions,
  type AuthorizationEpochResetEvent,
  type AuthorizationEpochResetHooks,
  type AuthorizationEpochSnapshot,
  type ReplicationSecurityState,
  type ReplicationSecurityStatePersistence,
} from "./security-reset.js";

export {
  MakoReplicationRecoveryCoordinator,
  MemoryReplicationRecoveryStatePersistence,
  type MakoReplicationRecoveryCoordinatorOptions,
  type MakoReplicationRecoveryHooks,
  type MakoReplicationRecoveryState,
  type ReplicationRecoveryStatePersistence,
} from "./recovery.js";

export {
  DEFAULT_REPLICATION_STATE_DATABASE,
  DexieReplicationStatePersistence,
  DexieReplicationStateStore,
  MemoryReplicationStateStore,
  type DexieReplicationStatePersistenceOptions,
  type DexieReplicationStatePersistenceScope,
  type DexieReplicationStateStoreOptions,
  type ReplicationCheckpointPersistence,
  type ReplicationStateStore,
} from "./replication-state.js";

export {
  MakoReplicationSignals,
  type MakoReplicationActivity,
  type MakoSanitizedReplicationError,
  type MakoThrottleSignal,
} from "./signals.js";

export {
  createMakoPushHandler,
  createMakoPushOptions,
  type MakoPushAdapterOptions,
} from "./push.js";

export {
  MakoReplicationError,
  makoReplicationErrorFrom,
  type ApiErrorEnvelope,
  type MakoReplicationErrorExtensions,
} from "./replication-error.js";
