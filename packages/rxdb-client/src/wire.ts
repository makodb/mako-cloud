/**
 * The Mako Cloud wire contract this package depends on, owned by this package.
 *
 * `@mako-cloud/rxdb` is published to npm on its own, so it cannot depend on the
 * repository's generated OpenAPI types: that package is unpublished, and its
 * `components` tree carries every management, operator, and service schema the
 * platform defines. Only the application-facing payloads an application client
 * actually exchanges are declared here, by hand.
 *
 * `test-types/conformance.ts` pins each declaration below against its generated
 * counterpart. It compiles only inside the monorepo, so a contract change that
 * would break this client fails there rather than in a customer's build.
 */

/* -------------------------------------------------------------------------- */
/* Scope identifiers                                                          */
/* -------------------------------------------------------------------------- */

declare const projectIdBrand: unique symbol;
declare const environmentIdBrand: unique symbol;
declare const collectionIdBrand: unique symbol;

/** A project identifier that has been checked against the service's format. */
export type ProjectId = string & { readonly [projectIdBrand]: true };
/** An environment identifier that has been checked against the service's format. */
export type EnvironmentId = string & { readonly [environmentIdBrand]: true };
/** A collection identifier that has been checked against the service's format. */
export type CollectionId = string & { readonly [collectionIdBrand]: true };

const PROJECT_ID = /^prj_[A-Za-z0-9_-]{8,64}$/;
const ENVIRONMENT_ID = /^env_[A-Za-z0-9_-]{8,64}$/;
const COLLECTION_ID = /^[a-z][a-z0-9_-]{0,62}$/;

/** Why an identifier was refused. */
export type ScopeValidationCode = "invalid_project" | "invalid_environment" | "invalid_collection";

/** Thrown when a project, environment, or collection identifier is malformed. */
export class ScopeValidationError extends Error {
  override readonly name: string = "ScopeValidationError";
  readonly code: ScopeValidationCode;

  constructor(code: ScopeValidationCode) {
    super(code.replaceAll("_", " "));
    this.code = code;
  }
}

export function parseProjectId(value: string): ProjectId {
  if (!PROJECT_ID.test(value)) {
    throw new ScopeValidationError("invalid_project");
  }
  return value as ProjectId;
}

export function parseEnvironmentId(value: string): EnvironmentId {
  if (!ENVIRONMENT_ID.test(value)) {
    throw new ScopeValidationError("invalid_environment");
  }
  return value as EnvironmentId;
}

export function parseCollectionId(value: string): CollectionId {
  if (!COLLECTION_ID.test(value)) {
    throw new ScopeValidationError("invalid_collection");
  }
  return value as CollectionId;
}

/* -------------------------------------------------------------------------- */
/* Error envelope                                                             */
/* -------------------------------------------------------------------------- */

export const API_ERROR_VERSION = "v1" as const;

/**
 * The error codes an application client can receive. The service also defines
 * `operator_step_up_required`, which belongs to the operator surface and is
 * unreachable from an application-user session or a public project key.
 */
export const ERROR_CODES = {
  INVALID_REQUEST: "invalid_request",
  UNAUTHENTICATED: "unauthenticated",
  PERMISSION_DENIED: "permission_denied",
  NOT_FOUND: "not_found",
  CONFLICT: "conflict",
  PRECONDITION_FAILED: "precondition_failed",
  SCHEMA_MISMATCH: "schema_mismatch",
  CHECKPOINT_EXPIRED: "checkpoint_expired",
  RATE_LIMITED: "rate_limited",
  QUOTA_EXCEEDED: "quota_exceeded",
  UNAVAILABLE: "unavailable",
  INTERNAL: "internal",
} as const;

export type ErrorCode = (typeof ERROR_CODES)[keyof typeof ERROR_CODES];

/** Whether, and how soon, the same request may be sent again. */
export type RetryAdvice =
  | { readonly kind: "never" }
  | { readonly kind: "immediate" }
  | { readonly kind: "after_delay"; readonly afterMs: number };

export type SafeDetail = string | boolean | number | readonly string[];
export type SafeDetails = Readonly<Record<string, SafeDetail>>;

export interface ApiError {
  readonly code: ErrorCode;
  readonly message: string;
  readonly requestId: string;
  readonly retry: RetryAdvice;
  readonly details?: SafeDetails;
}

/** The single shape every Mako Cloud endpoint uses to report a failure. */
export interface ApiErrorEnvelope {
  readonly apiVersion: typeof API_ERROR_VERSION;
  readonly error: ApiError;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Whether a value is one of the error codes this contract defines. */
export function isErrorCode(value: unknown): value is ErrorCode {
  return typeof value === "string" && (Object.values(ERROR_CODES) as string[]).includes(value);
}

/**
 * Whether a value is well-formed retry advice. The advice is the service's
 * explicit statement about repeating a request, so a client must be able to
 * recognize it before it falls back to guessing from the status code.
 */
export function isRetryAdvice(value: unknown): value is RetryAdvice {
  if (!isRecord(value)) {
    return false;
  }
  if (value.kind === "never" || value.kind === "immediate") {
    return true;
  }
  return (
    value.kind === "after_delay" &&
    typeof value.afterMs === "number" &&
    Number.isFinite(value.afterMs) &&
    value.afterMs >= 0
  );
}

/** A conservative boundary guard for responses received from an API. */
export function isApiErrorEnvelope(value: unknown): value is ApiErrorEnvelope {
  if (!isRecord(value) || value.apiVersion !== API_ERROR_VERSION || !isRecord(value.error)) {
    return false;
  }

  const { error } = value;
  return (
    isErrorCode(error.code) &&
    typeof error.message === "string" &&
    typeof error.requestId === "string" &&
    isRetryAdvice(error.retry)
  );
}

/* -------------------------------------------------------------------------- */
/* Project authentication payloads                                            */
/* -------------------------------------------------------------------------- */

/** The application user a session belongs to. */
export interface MakoAuthUser {
  readonly id: string;
  readonly email: string;
  readonly status: "unverified" | "active" | "disabled" | "deleted";
  /**
   * Bumped by the service whenever this user's authorization changes. A
   * replication scope compares it to decide that its local state must be reset.
   */
  readonly authorizationEpoch: number;
}

/** What every successful sign-in, exchange, redeem, and refresh returns. */
export interface MakoAuthSession {
  readonly accessToken: string;
  readonly refreshToken: string;
  /** Access-token lifetime in seconds, counted from the moment it was issued. */
  readonly expiresIn: number;
  readonly user: MakoAuthUser;
}

/** Sign-up is accepted without revealing whether the address was already registered. */
export interface MakoSignUpAccepted {
  readonly accepted: true;
}

/** Where to send the browser to begin an external provider sign-in. */
export interface MakoProviderSignInStart {
  readonly authorizationUrl: string;
  readonly provider: string;
}

/** A magic link request is accepted without revealing whether the address exists. */
export interface MakoMagicLinkAccepted {
  readonly accepted: true;
}

/* -------------------------------------------------------------------------- */
/* Storage object payloads                                                    */
/* -------------------------------------------------------------------------- */

/** The record the service returns for a stored or listed object. */
export interface MakoStorageObjectRecord {
  readonly bucketId: string;
  readonly path: string;
  readonly contentType: string;
  readonly sizeBytes: number;
  /** The application user that wrote the object, or `null` for an anonymous write. */
  readonly ownerId: string | null;
  /** `sha256:` of the plaintext, what a client can verify. */
  readonly digest: string;
  /** `sha256:` of what the object store holds. */
  readonly storedDigest: string;
  readonly createdAtUnixSeconds: number;
  readonly updatedAtUnixSeconds: number;
  /**
   * What the application attached when the object was stored. The bucket's
   * rules read these -- `old.attributes.household_id` -- which is how an
   * object belongs to something other than its uploader.
   */
  readonly attributes: Record<string, string>;
}

/** One page of a bucket listing; `nextCursor` is `null` on the last page. */
export interface MakoStorageObjectPage {
  readonly items: MakoStorageObjectRecord[];
  readonly nextCursor: string | null;
}

/* -------------------------------------------------------------------------- */
/* Replication payloads                                                       */
/* -------------------------------------------------------------------------- */

/** A replicated document as it travels on the wire. */
export interface MakoJsonDocument {
  [key: string]: unknown;
}

/** A replicated document as RxDB requires it: the wire document plus its tombstone flag. */
export interface MakoReplicationDocument extends MakoJsonDocument {
  _deleted: boolean;
}

export interface MakoPullResponse {
  readonly documents: MakoJsonDocument[];
  /** An opaque resume token; never parsed by a client. */
  readonly checkpoint: string;
}

export interface MakoPushRow {
  readonly mutationId: string;
  readonly assumedMasterState?: MakoJsonDocument | null;
  readonly newDocumentState: MakoJsonDocument;
}

export interface MakoPushOutcome {
  readonly mutationId: string;
  readonly status: "accepted" | "conflict" | "denied";
  /** Present on `conflict`: the state the service holds, for RxDB to resolve against. */
  readonly masterState?: MakoJsonDocument;
  /** Present on `denied`: why a document policy refused the write. */
  readonly error?: ApiErrorEnvelope;
}

export interface MakoPushResponse {
  readonly outcomes: MakoPushOutcome[];
}

/** Why the live stream asked its subscriber to resynchronize. */
export type MakoResyncReason =
  | "reconnected"
  | "stream_gap"
  | "checkpoint_expired"
  | "authorization_epoch_changed"
  | "service_failover";

/** One server-sent event on the replication stream. */
export type MakoLiveStreamEvent =
  | {
      readonly event: "documents";
      readonly data: {
        readonly documents: MakoJsonDocument[];
        readonly checkpoint: string;
        readonly cursor: string;
      };
    }
  | {
      readonly event: "checkpoint";
      readonly data: { readonly checkpoint: string; readonly cursor: string };
    }
  | { readonly event: "heartbeat"; readonly data: { readonly cursor: string } }
  | { readonly event: "resync"; readonly data: { readonly reason: MakoResyncReason } };
