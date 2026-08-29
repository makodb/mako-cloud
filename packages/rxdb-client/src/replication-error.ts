import {
  ERROR_CODES,
  isApiErrorEnvelope,
  isErrorCode,
  isRetryAdvice,
  type ApiError,
  type ApiErrorEnvelope,
  type ErrorCode,
  type RetryAdvice,
  type SafeDetails,
} from "./wire.js";

/**
 * What a `MakoReplicationError` carries through RxDB.
 *
 * RxDB wraps whatever a pull or push handler throws in an `RC_PULL` / `RC_PUSH`
 * `RxError` and converts the original with `errorToPlainJson`, which keeps only
 * `name`, `message`, `code`, `url`, `parameters`, `extensions`, and `stack`.
 * Everything a client needs to decide what to do -- the request id, the
 * service's retry advice, and the status -- therefore travels in `extensions`,
 * so `makoReplicationErrorFrom` can rebuild the error an application observes
 * on `error$`.
 */
export interface MakoReplicationErrorExtensions {
  readonly requestId: string;
  readonly retry: RetryAdvice;
  readonly retryable: boolean;
  readonly retryAfterMilliseconds: number | null;
  readonly status: number | null;
  readonly details: SafeDetails | null;
}

export class MakoReplicationError extends Error {
  override readonly name: string = "MakoReplicationError";
  readonly code: ErrorCode;
  readonly requestId: string;
  readonly retry: RetryAdvice;
  readonly details?: SafeDetails;
  readonly status?: number;
  /**
   * Whether repeating the request can succeed. The service's advice decides:
   * `never` is terminal whatever the status was. A refused credential is
   * terminal too -- repeating a request with a credential the service has
   * already rejected is what turns an ended session into a request storm.
   */
  readonly retryable: boolean;
  /**
   * How long to wait before repeating, from an `after_delay` advice; `null`
   * when the service named no delay.
   */
  readonly retryAfterMilliseconds: number | null;
  readonly extensions: MakoReplicationErrorExtensions;

  constructor(error: ApiError, status?: number) {
    super(error.message);
    this.code = error.code;
    this.requestId = error.requestId;
    this.retry = error.retry;
    if (error.details !== undefined) {
      this.details = error.details;
    }
    if (status !== undefined) {
      this.status = status;
    }
    this.retryAfterMilliseconds = error.retry.kind === "after_delay" ? error.retry.afterMs : null;
    this.retryable = isRetryable(error.code, error.retry);
    this.extensions = {
      requestId: this.requestId,
      retry: this.retry,
      retryable: this.retryable,
      retryAfterMilliseconds: this.retryAfterMilliseconds,
      status: status ?? null,
      details: error.details ?? null,
    };
  }
}

/**
 * Whether the service definitively refused the credential the request carried.
 * This is the replication-boundary spelling of `isDefinitiveRefusal` in
 * `auth.ts`: a `401`, or a `403` the service labelled `unauthenticated`. A
 * `403 permission_denied` is a policy decision about a document, not a verdict
 * on the session, and a `408`, `429`, or `5xx` says nothing about it at all.
 */
export function isAuthenticationRefusal(status: number, code?: ErrorCode): boolean {
  return status === 401 || (status === 403 && code === ERROR_CODES.UNAUTHENTICATED);
}

export async function replicationResponseError(response: Response): Promise<MakoReplicationError> {
  const body: unknown = await response.json().catch(() => null);
  const envelope = isApiErrorEnvelope(body) ? body.error : null;
  if (isAuthenticationRefusal(response.status, envelope?.code)) {
    return new MakoReplicationError(
      {
        code: ERROR_CODES.UNAUTHENTICATED,
        message: envelope?.message ?? "the application-user session was refused",
        requestId: envelope?.requestId ?? requestIdFrom(response),
        // A refused credential is never repeatable, whatever the envelope said.
        retry: { kind: "never" },
        ...(envelope?.details === undefined ? {} : { details: envelope.details }),
      },
      response.status,
    );
  }
  if (envelope !== null) {
    return new MakoReplicationError(envelope, response.status);
  }
  return new MakoReplicationError(
    {
      code: response.status >= 500 ? ERROR_CODES.UNAVAILABLE : ERROR_CODES.INTERNAL,
      message: "replication request failed",
      requestId: requestIdFrom(response),
      retry: statusOnlyAdvice(response.status),
    },
    response.status,
  );
}

export function replicationNetworkError(): MakoReplicationError {
  return new MakoReplicationError({
    code: ERROR_CODES.UNAVAILABLE,
    message: "replication service is unavailable",
    requestId: "req_unavailable",
    retry: { kind: "immediate" },
  });
}

/**
 * A refresh that could not reach a verdict: the session is intact, so the
 * replication should retry rather than report `unauthenticated` and drive the
 * application into a signed-out state.
 */
export function sessionRefreshUnavailableError(): MakoReplicationError {
  return new MakoReplicationError({
    code: ERROR_CODES.UNAVAILABLE,
    message: "the application-user session could not be refreshed",
    requestId: "req_client",
    retry: { kind: "after_delay", afterMs: 1_000 },
  });
}

export function authenticationRequiredError(): MakoReplicationError {
  return new MakoReplicationError({
    code: ERROR_CODES.UNAUTHENTICATED,
    message: "an application-user session is required",
    requestId: "req_client",
    retry: { kind: "never" },
  });
}

/**
 * The `MakoReplicationError` behind a value, including one RxDB has wrapped in
 * an `RC_PULL` / `RC_PUSH` error and flattened to plain JSON, or `null` when
 * the value is not one of this client's failures. Only the fields this package
 * writes are read back, so an unrecognized error never leaks its content.
 */
export function makoReplicationErrorFrom(error: unknown): MakoReplicationError | null {
  if (error instanceof MakoReplicationError) {
    return error;
  }
  for (const candidate of wrappedErrors(error)) {
    const rebuilt = rebuildReplicationError(candidate);
    if (rebuilt !== null) {
      return rebuilt;
    }
  }
  return null;
}

function wrappedErrors(error: unknown): readonly unknown[] {
  if (!isRecord(error) || error.rxdb !== true || !isRecord(error.parameters)) {
    return [];
  }
  const { errors } = error.parameters;
  return Array.isArray(errors) ? errors : [];
}

function rebuildReplicationError(candidate: unknown): MakoReplicationError | null {
  if (
    !isRecord(candidate) ||
    candidate.name !== "MakoReplicationError" ||
    !isErrorCode(candidate.code) ||
    typeof candidate.message !== "string" ||
    !isRecord(candidate.extensions)
  ) {
    return null;
  }
  const extensions = candidate.extensions;
  if (typeof extensions.requestId !== "string" || !isRetryAdvice(extensions.retry)) {
    return null;
  }
  const status = extensions.status;
  return new MakoReplicationError(
    {
      code: candidate.code,
      message: candidate.message,
      requestId: extensions.requestId,
      retry: extensions.retry,
      ...(isRecord(extensions.details) ? { details: extensions.details as SafeDetails } : {}),
    },
    typeof status === "number" ? status : undefined,
  );
}

function isRetryable(code: ErrorCode, retry: RetryAdvice): boolean {
  if (retry.kind === "never") {
    return false;
  }
  return code !== ERROR_CODES.UNAUTHENTICATED;
}

/**
 * The fallback for a failure that carried no error envelope -- a proxy, a load
 * balancer, or a gateway answering for the service. It is a guess, and any
 * advice the service did send outranks it.
 */
function statusOnlyAdvice(status: number): RetryAdvice {
  if (status >= 500) {
    return { kind: "immediate" };
  }
  if (status === 408 || status === 429) {
    return { kind: "after_delay", afterMs: 1_000 };
  }
  return { kind: "never" };
}

function requestIdFrom(response: Response): string {
  const value = response.headers.get("x-request-id");
  return value !== null && value.length <= 128 ? value : "req_unknown";
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export type { ApiErrorEnvelope };
