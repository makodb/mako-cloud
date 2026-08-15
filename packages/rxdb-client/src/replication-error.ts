import {
  isApiErrorEnvelope,
  type ApiError,
  type ApiErrorEnvelope,
  type ErrorCode,
  type RetryAdvice,
  type SafeDetails,
} from "@mako-cloud/api-types";

export class MakoReplicationError extends Error {
  override readonly name: string = "MakoReplicationError";
  readonly code: ErrorCode;
  readonly requestId: string;
  readonly retry: RetryAdvice;
  readonly details?: SafeDetails;
  readonly status?: number;

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
  }

  get retryable(): boolean {
    return this.retry.kind !== "never";
  }
}

export async function replicationResponseError(response: Response): Promise<MakoReplicationError> {
  const body: unknown = await response.json().catch(() => null);
  if (isApiErrorEnvelope(body)) {
    return new MakoReplicationError(body.error, response.status);
  }
  return new MakoReplicationError(
    {
      code: "internal",
      message: "replication request failed",
      requestId: requestIdFrom(response),
      retry: { kind: response.status >= 500 ? "immediate" : "never" },
    },
    response.status,
  );
}

export function replicationNetworkError(): MakoReplicationError {
  return new MakoReplicationError({
    code: "unavailable",
    message: "replication service is unavailable",
    requestId: "req_unavailable",
    retry: { kind: "immediate" },
  });
}

export function authenticationRequiredError(): MakoReplicationError {
  return new MakoReplicationError({
    code: "unauthenticated",
    message: "an application-user session is required",
    requestId: "req_client",
    retry: { kind: "never" },
  });
}

function requestIdFrom(response: Response): string {
  const value = response.headers.get("x-request-id");
  return value !== null && value.length <= 128 ? value : "req_unknown";
}

export type { ApiErrorEnvelope };
