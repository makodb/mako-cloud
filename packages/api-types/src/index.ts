/** Shared generated and hand-written wire types. */

export { createMakoApiClient } from "./client.js";
export type { MakoApiClient } from "./client.js";
export type { components, operations, paths } from "./generated/schema.js";
export {
  ScopeValidationError,
  assertSameTenant,
  parseCollectionId,
  parseEnvironmentId,
  parseProjectId,
  requireCollectionScope,
  requireTenantScope,
} from "./scope.js";
export type {
  CollectionId,
  CollectionScope,
  EnvironmentId,
  ProjectId,
  TenantScope,
} from "./scope.js";

export const PACKAGE_NAME = "@mako-cloud/api-types" as const;
export const API_ERROR_VERSION = "v1" as const;

export const ERROR_CODES = {
  INVALID_REQUEST: "invalid_request",
  UNAUTHENTICATED: "unauthenticated",
  PERMISSION_DENIED: "permission_denied",
  NOT_FOUND: "not_found",
  CONFLICT: "conflict",
  SCHEMA_MISMATCH: "schema_mismatch",
  CHECKPOINT_EXPIRED: "checkpoint_expired",
  RATE_LIMITED: "rate_limited",
  QUOTA_EXCEEDED: "quota_exceeded",
  UNAVAILABLE: "unavailable",
  INTERNAL: "internal",
} as const;

export type ErrorCode = (typeof ERROR_CODES)[keyof typeof ERROR_CODES];

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

export interface ApiErrorEnvelope {
  readonly apiVersion: typeof API_ERROR_VERSION;
  readonly error: ApiError;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** A conservative boundary guard for responses received from an API. */
export function isApiErrorEnvelope(value: unknown): value is ApiErrorEnvelope {
  if (!isRecord(value) || value.apiVersion !== API_ERROR_VERSION || !isRecord(value.error)) {
    return false;
  }

  const { error } = value;
  return (
    typeof error.code === "string" &&
    Object.values(ERROR_CODES).includes(error.code as ErrorCode) &&
    typeof error.message === "string" &&
    typeof error.requestId === "string" &&
    isRecord(error.retry) &&
    (error.retry.kind === "never" ||
      error.retry.kind === "immediate" ||
      (error.retry.kind === "after_delay" && typeof error.retry.afterMs === "number"))
  );
}
