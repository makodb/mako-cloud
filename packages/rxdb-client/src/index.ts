/** RxDB replication and project-auth client. */
export {
  createMakoApiClient as createReplicationClient,
  type MakoApiClient as ReplicationClient,
} from "@mako-cloud/api-types";

export const PACKAGE_NAME = "@mako-cloud/rxdb" as const;
export const PACKAGE_VERSION = "0.1.0" as const;

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
  type MakoUserSession,
  type PersistedAuthSession,
} from "./auth.js";

export {
  MakoRxdbConfigurationError,
  SUPPORTED_RXDB_MAJOR,
  SUPPORTED_RXDB_RANGE,
  UnsupportedRxdbVersionError,
  assertSupportedRxdbVersion,
  normalizeMakoRxdbConfig,
  type MakoRxdbClientConfig,
  type MakoRxdbRuntime,
  type NormalizedMakoRxdbClientConfig,
} from "./config.js";

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
  type MakoReplicationRecoveryHooks,
  type MakoReplicationRecoveryState,
} from "./recovery.js";

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
  type ApiErrorEnvelope,
} from "./replication-error.js";
