import { MakoAuthError, MakoAuthenticationRequiredError, type MakoAuthClient } from "./auth.js";
import type { NormalizedMakoRxdbClientConfig } from "./config.js";
import {
  isApiErrorEnvelope,
  type ApiError,
  type ErrorCode,
  type MakoStorageObjectPage,
  type MakoStorageObjectRecord,
  type RetryAdvice,
  type SafeDetails,
} from "./wire.js";

export type { MakoStorageObjectPage, MakoStorageObjectRecord };

export type MakoStorageScope = Pick<
  NormalizedMakoRxdbClientConfig,
  "endpoint" | "projectId" | "environmentId" | "publicProjectKey"
>;

export interface MakoStorageClientOptions {
  readonly fetch?: typeof globalThis.fetch;
}

export type MakoStorageBody = Blob | ArrayBuffer | Uint8Array | string;

export interface MakoStoragePutOptions {
  /** Recorded with the object and checked against the bucket's allowed types. */
  readonly contentType: string;
  /** Sent verbatim as `If-None-Match` (for example `*` to refuse overwriting). */
  readonly ifNoneMatch?: string;
}

export interface MakoStoragePutResult {
  /** The ETag a later `get` answers with: the quoted plaintext digest. */
  readonly etag: string;
  readonly size: number;
}

export interface MakoStorageObject {
  readonly bytes: Uint8Array;
  readonly contentType: string;
  readonly etag: string;
}

export interface MakoStorageListOptions {
  readonly prefix?: string;
  readonly limit?: number;
  readonly cursor?: string;
}

export interface MakoStorageListResult {
  readonly items: readonly MakoStorageObjectRecord[];
  readonly nextCursor: string | null;
}

export class MakoStorageError extends Error {
  override readonly name: string = "MakoStorageError";
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

const MAXIMUM_PATH_BYTES = 512;
const MAXIMUM_LIST_LIMIT = 1_000;

/**
 * Application access to bucket objects under the same public key and
 * application-user session replication uses. Writes require a session;
 * reads carry one when it exists so a policy bucket can evaluate it and a
 * public bucket answers without it.
 */
export class MakoStorageClient {
  readonly #scope: MakoStorageScope;
  readonly #auth: MakoAuthClient;
  readonly #fetch: typeof globalThis.fetch;

  constructor(
    scope: MakoStorageScope,
    auth: MakoAuthClient,
    options: MakoStorageClientOptions = {},
  ) {
    this.#scope = scope;
    this.#auth = auth;
    this.#fetch = options.fetch ?? globalThis.fetch;
  }

  /** The object's URL, for `<img src>` and links on a public bucket. */
  url(bucketId: string, path: string): string {
    return this.#objectUrl(bucketId, path).toString();
  }

  async put(
    bucketId: string,
    path: string,
    body: MakoStorageBody,
    options: MakoStoragePutOptions,
  ): Promise<MakoStoragePutResult> {
    if (options.contentType.length < 1 || hasControlCharacters(options.contentType)) {
      throw clientError("invalid_request", "contentType is invalid");
    }
    const url = this.#objectUrl(bucketId, path);
    const headers: Record<string, string> = {
      Accept: "application/json",
      Authorization: `Bearer ${await this.#requiredToken()}`,
      "Content-Type": options.contentType,
      "X-Mako-Key": this.#scope.publicProjectKey,
    };
    if (options.ifNoneMatch !== undefined) {
      headers["If-None-Match"] = options.ifNoneMatch;
    }
    const response = await this.#send(url, { method: "PUT", headers, body: bodyInit(body) });
    if (!response.ok) {
      throw await storageResponseError(response);
    }
    const record: unknown = await response.json().catch(() => null);
    if (!isObjectRecord(record)) {
      throw malformedResponse(response);
    }
    return { etag: `"${record.digest}"`, size: record.sizeBytes };
  }

  async get(bucketId: string, path: string): Promise<MakoStorageObject | null> {
    const url = this.#objectUrl(bucketId, path);
    const response = await this.#send(url, {
      method: "GET",
      headers: {
        Accept: "*/*",
        "X-Mako-Key": this.#scope.publicProjectKey,
        ...(await this.#optionalBearer()),
      },
    });
    if (response.status === 404) {
      return null;
    }
    if (!response.ok) {
      throw await storageResponseError(response);
    }
    const bytes = new Uint8Array(await response.arrayBuffer());
    return {
      bytes,
      contentType: response.headers.get("content-type") ?? "application/octet-stream",
      etag: response.headers.get("etag") ?? "",
    };
  }

  async list(
    bucketId: string,
    options: MakoStorageListOptions = {},
  ): Promise<MakoStorageListResult> {
    const url = this.#objectsUrl(bucketId);
    if (options.prefix !== undefined) {
      if (byteLength(options.prefix) > MAXIMUM_PATH_BYTES || hasControlCharacters(options.prefix)) {
        throw clientError("invalid_request", "prefix is invalid");
      }
      url.searchParams.set("prefix", options.prefix);
    }
    if (options.limit !== undefined) {
      if (
        !Number.isSafeInteger(options.limit) ||
        options.limit < 1 ||
        options.limit > MAXIMUM_LIST_LIMIT
      ) {
        throw clientError("invalid_request", `limit must be between 1 and ${MAXIMUM_LIST_LIMIT}`);
      }
      url.searchParams.set("limit", String(options.limit));
    }
    if (options.cursor !== undefined) {
      if (options.cursor.length < 1 || options.cursor.length > MAXIMUM_PATH_BYTES) {
        throw clientError("invalid_request", "cursor is invalid");
      }
      url.searchParams.set("cursor", options.cursor);
    }
    const response = await this.#send(url, {
      method: "GET",
      headers: {
        Accept: "application/json",
        "X-Mako-Key": this.#scope.publicProjectKey,
        ...(await this.#optionalBearer()),
      },
    });
    if (!response.ok) {
      throw await storageResponseError(response);
    }
    const page: unknown = await response.json().catch(() => null);
    if (!isObjectPage(page)) {
      throw malformedResponse(response);
    }
    return { items: page.items, nextCursor: page.nextCursor };
  }

  async delete(bucketId: string, path: string): Promise<void> {
    const url = this.#objectUrl(bucketId, path);
    const response = await this.#send(url, {
      method: "DELETE",
      headers: {
        Accept: "application/json",
        Authorization: `Bearer ${await this.#requiredToken()}`,
        "X-Mako-Key": this.#scope.publicProjectKey,
      },
    });
    if (!response.ok) {
      throw await storageResponseError(response);
    }
    await response.arrayBuffer().catch(() => undefined);
  }

  #objectsUrl(bucketId: string): URL {
    if (bucketId.length < 1 || bucketId.length > 128 || hasControlCharacters(bucketId)) {
      throw clientError("invalid_request", "bucketId is invalid");
    }
    const base = new URL(this.#scope.endpoint);
    base.pathname = `${base.pathname.replace(/\/+$/u, "")}/`;
    return new URL(
      `v1/projects/${encodeURIComponent(this.#scope.projectId)}/environments/${encodeURIComponent(
        this.#scope.environmentId,
      )}/storage/${encodeURIComponent(bucketId)}/objects`,
      base,
    );
  }

  #objectUrl(bucketId: string, path: string): URL {
    const url = this.#objectsUrl(bucketId);
    url.pathname = `${url.pathname}/${encodeObjectPath(path)}`;
    return url;
  }

  async #requiredToken(): Promise<string> {
    try {
      return await this.#auth.validAccessToken();
    } catch (error) {
      throw sessionError(error);
    }
  }

  async #optionalBearer(): Promise<Record<string, string>> {
    try {
      return { Authorization: `Bearer ${await this.#auth.validAccessToken()}` };
    } catch (error) {
      if (error instanceof MakoAuthenticationRequiredError) {
        return {};
      }
      throw sessionError(error);
    }
  }

  async #send(url: URL, init: RequestInit): Promise<Response> {
    try {
      return await this.#fetch(url, init);
    } catch {
      throw new MakoStorageError({
        code: "unavailable",
        message: "storage service is unavailable",
        requestId: "req_unavailable",
        retry: { kind: "immediate" },
      });
    }
  }
}

/** Percent-encode an object path segment by segment, refusing what can never be a valid path. */
export function encodeObjectPath(path: string): string {
  const bytes = byteLength(path);
  if (bytes < 1 || bytes > MAXIMUM_PATH_BYTES || hasControlCharacters(path)) {
    throw clientError("invalid_request", "object path is invalid");
  }
  const segments = path.split("/");
  if (segments.some((segment) => segment.length === 0 || segment === "." || segment === "..")) {
    throw clientError("invalid_request", "object path segments must be non-empty and not . or ..");
  }
  return segments.map((segment) => encodeURIComponent(segment)).join("/");
}

function bodyInit(body: MakoStorageBody): BodyInit {
  if (body instanceof Uint8Array) {
    const buffer = body.buffer;
    if (buffer instanceof ArrayBuffer) {
      return new Uint8Array(buffer, body.byteOffset, body.byteLength);
    }
    const copy = new Uint8Array(body.byteLength);
    copy.set(body);
    return copy;
  }
  return body;
}

async function storageResponseError(response: Response): Promise<MakoStorageError> {
  const body: unknown = await response.json().catch(() => null);
  if (isApiErrorEnvelope(body)) {
    return new MakoStorageError(body.error, response.status);
  }
  return new MakoStorageError(
    {
      code: response.status >= 500 ? "unavailable" : "internal",
      message: "storage request failed",
      requestId: requestIdFrom(response),
      retry: { kind: response.status >= 500 ? "immediate" : "never" },
    },
    response.status,
  );
}

function malformedResponse(response: Response): MakoStorageError {
  return new MakoStorageError(
    {
      code: "internal",
      message: "storage response is malformed",
      requestId: requestIdFrom(response),
      retry: { kind: "never" },
    },
    response.status,
  );
}

/**
 * A missing or refused session is `unauthenticated`; a refresh that could not
 * reach the service keeps the session and is a retryable `unavailable`.
 */
function sessionError(error: unknown): MakoStorageError {
  if (error instanceof MakoAuthError && error.retryable) {
    return new MakoStorageError({
      code: "unavailable",
      message: "the application-user session could not be refreshed",
      requestId: "req_client",
      retry: { kind: "after_delay", afterMs: 1_000 },
    });
  }
  return clientError("unauthenticated", "an application-user session is required");
}

function clientError(
  code: "invalid_request" | "unauthenticated",
  message: string,
): MakoStorageError {
  return new MakoStorageError({
    code,
    message,
    requestId: "req_client",
    retry: { kind: "never" },
  });
}

function requestIdFrom(response: Response): string {
  const value = response.headers.get("x-request-id");
  return value !== null && value.length <= 128 ? value : "req_unknown";
}

function isObjectRecord(value: unknown): value is MakoStorageObjectRecord {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const candidate = value as { path?: unknown; digest?: unknown; sizeBytes?: unknown };
  return (
    typeof candidate.path === "string" &&
    typeof candidate.digest === "string" &&
    typeof candidate.sizeBytes === "number" &&
    Number.isSafeInteger(candidate.sizeBytes) &&
    candidate.sizeBytes >= 0
  );
}

function isObjectPage(value: unknown): value is MakoStorageObjectPage {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const candidate = value as { items?: unknown; nextCursor?: unknown };
  return (
    Array.isArray(candidate.items) &&
    candidate.items.every(isObjectRecord) &&
    (candidate.nextCursor === null || typeof candidate.nextCursor === "string")
  );
}

function byteLength(value: string): number {
  return new TextEncoder().encode(value).length;
}

function hasControlCharacters(value: string): boolean {
  return Array.from(value).some((character) => {
    const codePoint = character.codePointAt(0);
    return codePoint !== undefined && (codePoint <= 0x1f || codePoint === 0x7f);
  });
}
