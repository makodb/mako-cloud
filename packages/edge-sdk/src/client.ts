import {
  isApiErrorEnvelope,
  parseCollectionId,
  parseEnvironmentId,
  parseProjectId,
  type ApiErrorEnvelope,
  type ErrorCode,
  type RetryAdvice,
  type components,
} from "@mako-cloud/api-types";

export type JsonObject = Record<string, unknown>;
type AuthUser = components["schemas"]["AuthUser"];
type WireDocument = components["schemas"]["DocumentRecord"];
type WireMutation = components["schemas"]["DocumentMutationRequest"];
type WireMutationResult = components["schemas"]["DocumentMutationResult"];
export type FunctionDocumentQuery = components["schemas"]["DocumentQueryRequest"];

export type FunctionDocument<T extends JsonObject = JsonObject> = Omit<WireDocument, "body"> & {
  readonly body: T;
};

export type FunctionDocumentMutation<T extends JsonObject = JsonObject> = Omit<
  WireMutation,
  "body"
> & {
  readonly body: T;
};

export type FunctionDocumentMutationResult<T extends JsonObject = JsonObject> = Omit<
  WireMutationResult,
  "document"
> & {
  readonly document: FunctionDocument<T> | null;
};

export interface FunctionDocumentQueryPage<T extends JsonObject = JsonObject> {
  readonly documents: FunctionDocument<T>[];
  readonly nextCursor: string | null;
}

export interface FunctionClientOptions {
  /** Internal Mako API origin injected by the runtime. */
  readonly endpoint: string | URL;
  readonly projectId: string;
  readonly environmentId: string;
  /** Verified raw caller token from the runtime, or null for a public invocation. */
  readonly callerAuthorization: string | null;
  readonly requestId?: string;
  readonly fetch?: typeof globalThis.fetch;
}

export interface RuntimeFunctionClientOptions
  extends Omit<FunctionClientOptions, "callerAuthorization" | "requestId"> {
  /** Function request supplied by the hosted or local Mako runtime. */
  readonly request: Request;
}

export interface ServiceFunctionClientOptions {
  /** Internal Mako API origin injected by the runtime. */
  readonly endpoint: string | URL;
  readonly projectId: string;
  readonly environmentId: string;
  /** A service credential read from an explicitly attached function secret. */
  readonly serviceCredential: string;
  /** Required audit reason recorded for every privileged operation. */
  readonly reason: string;
  /** Runtime request identifier used to correlate the bypass audit event. */
  readonly requestId: string;
  readonly fetch?: typeof globalThis.fetch;
}

export interface FunctionAuthClient {
  /** Returns the application user represented by the verified invoking token. */
  getUser(): Promise<AuthUser>;
  /** Revokes the session represented by the verified invoking token. */
  signOut(): Promise<void>;
}

export interface FunctionDocumentClient<T extends JsonObject = JsonObject> {
  get(documentId: string): Promise<FunctionDocument<T> | null>;
  query(query: FunctionDocumentQuery): Promise<FunctionDocumentQueryPage<T>>;
  mutate(
    documentId: string,
    mutation: FunctionDocumentMutation<T>,
  ): Promise<FunctionDocumentMutationResult<T>>;
}

export interface FunctionClient {
  readonly auth: FunctionAuthClient;
  documents<T extends JsonObject = JsonObject>(collectionId: string): FunctionDocumentClient<T>;
}

export interface ServiceFunctionClient {
  documents<T extends JsonObject = JsonObject>(collectionId: string): FunctionDocumentClient<T>;
}

export class MakoEdgeSdkError extends Error {
  override readonly name: string = "MakoEdgeSdkError";
  readonly code?: ErrorCode;
  readonly requestId?: string;
  readonly retry?: RetryAdvice;
  readonly status?: number;

  constructor(
    message: string,
    options: {
      readonly apiError?: ApiErrorEnvelope;
      readonly status?: number;
    } = {},
  ) {
    super(message);
    if (options.apiError !== undefined) {
      this.code = options.apiError.error.code;
      this.requestId = options.apiError.error.requestId;
      this.retry = options.apiError.error.retry;
    }
    if (options.status !== undefined) {
      this.status = options.status;
    }
  }
}

export class MakoCallerIdentityRequiredError extends MakoEdgeSdkError {
  override readonly name: string = "MakoCallerIdentityRequiredError";
}

/**
 * Creates a tenant-bound client whose auth and document calls always forward
 * the verified invoking identity. Public invocations fail closed on first use.
 */
export function createFunctionClient(options: FunctionClientOptions): FunctionClient {
  const transport = new MakoTransport(options, {
    kind: "caller",
    value: validateCallerAuthorization(options.callerAuthorization),
  });
  return {
    auth: {
      getUser: () => transport.request<AuthUser>("auth/user", "GET"),
      signOut: () => transport.request<void>("auth/signout", "POST"),
    },
    documents: <T extends JsonObject = JsonObject>(collectionId: string) =>
      new DocumentClient<T>(transport, "collections", parseCollectionId(collectionId)),
  };
}

/** Creates the default caller client directly from the trusted runtime headers. */
export function createFunctionClientFromRequest(
  options: RuntimeFunctionClientOptions,
): FunctionClient {
  const callerAuthorization = options.request.headers.get("x-mako-caller-authorization");
  const requestId = options.request.headers.get("x-mako-request-id") ?? undefined;
  return createFunctionClient({
    endpoint: options.endpoint,
    projectId: options.projectId,
    environmentId: options.environmentId,
    callerAuthorization,
    ...(requestId === undefined ? {} : { requestId }),
    ...(options.fetch === undefined ? {} : { fetch: options.fetch }),
  });
}

/**
 * Explicitly creates a privileged document client from an attached service
 * credential. The dedicated service route verifies scope and audits the reason.
 */
export function createServiceClient(options: ServiceFunctionClientOptions): ServiceFunctionClient {
  const transport = new MakoTransport(options, {
    kind: "service",
    value: validateServiceCredential(options.serviceCredential),
    reason: validateBypassReason(options.reason),
  });
  return {
    documents: <T extends JsonObject = JsonObject>(collectionId: string) =>
      new DocumentClient<T>(transport, "service/collections", parseCollectionId(collectionId)),
  };
}

interface Transport {
  request<T>(
    path: string,
    method: "GET" | "POST",
    body?: unknown,
    extraHeaders?: Readonly<Record<string, string>>,
  ): Promise<T>;
}

class DocumentClient<T extends JsonObject> implements FunctionDocumentClient<T> {
  readonly #transport: Transport;
  readonly #routePrefix: string;
  readonly #collectionId: string;

  constructor(transport: Transport, routePrefix: string, collectionId: string) {
    this.#transport = transport;
    this.#routePrefix = routePrefix;
    this.#collectionId = collectionId;
  }

  async get(documentId: string): Promise<FunctionDocument<T> | null> {
    const path = this.#documentPath(documentId);
    try {
      return await this.#transport.request<FunctionDocument<T>>(path, "GET");
    } catch (error) {
      if (error instanceof MakoEdgeSdkError && error.status === 404) {
        return null;
      }
      throw error;
    }
  }

  query(query: FunctionDocumentQuery): Promise<FunctionDocumentQueryPage<T>> {
    validateQuery(query);
    return this.#transport.request<FunctionDocumentQueryPage<T>>(
      `${this.#routePrefix}/${encodeURIComponent(this.#collectionId)}/documents/query`,
      "POST",
      query,
    );
  }

  mutate(
    documentId: string,
    mutation: FunctionDocumentMutation<T>,
  ): Promise<FunctionDocumentMutationResult<T>> {
    validateMutation(mutation);
    return this.#transport.request<FunctionDocumentMutationResult<T>>(
      this.#documentPath(documentId),
      "POST",
      mutation,
      { "Idempotency-Key": mutation.mutationId },
    );
  }

  #documentPath(documentId: string): string {
    validateDocumentId(documentId);
    return `${this.#routePrefix}/${encodeURIComponent(
      this.#collectionId,
    )}/documents/${encodeURIComponent(documentId)}`;
  }
}

type TransportCredential =
  | { readonly kind: "caller"; readonly value: string | null }
  | { readonly kind: "service"; readonly value: string; readonly reason: string };

class MakoTransport implements Transport {
  readonly #endpoint: URL;
  readonly #projectId: string;
  readonly #environmentId: string;
  readonly #credential: TransportCredential;
  readonly #requestId: string | undefined;
  readonly #fetch: typeof globalThis.fetch;

  constructor(
    options: FunctionClientOptions | ServiceFunctionClientOptions,
    credential: TransportCredential,
  ) {
    this.#endpoint = parseEndpoint(options.endpoint);
    this.#projectId = parseProjectId(options.projectId);
    this.#environmentId = parseEnvironmentId(options.environmentId);
    this.#credential = credential;
    this.#requestId = validateRequestId(options.requestId);
    this.#fetch = options.fetch ?? globalThis.fetch;
  }

  async request<T>(
    path: string,
    method: "GET" | "POST",
    body?: unknown,
    extraHeaders: Readonly<Record<string, string>> = {},
  ): Promise<T> {
    if (this.#credential.kind === "caller" && this.#credential.value === null) {
      throw new MakoCallerIdentityRequiredError(
        "this operation requires an authenticated function caller",
      );
    }
    const headers = new Headers({
      Accept: "application/json",
      ...extraHeaders,
    });
    if (this.#credential.kind === "caller") {
      headers.set("Authorization", `Bearer ${this.#credential.value}`);
    } else {
      headers.set("X-Mako-Service-Key", this.#credential.value);
      headers.set("X-Mako-Bypass-Reason", this.#credential.reason);
    }
    if (body !== undefined) {
      headers.set("Content-Type", "application/json");
    }
    if (this.#requestId !== undefined) {
      headers.set("X-Mako-Request-Id", this.#requestId);
    }
    let encodedBody: string | undefined;
    try {
      encodedBody = body === undefined ? undefined : JSON.stringify(body);
    } catch {
      throw new MakoEdgeSdkError("function SDK request is not JSON serializable");
    }
    let response: Response;
    try {
      const requestInit: RequestInit = { method, headers };
      if (encodedBody !== undefined) {
        requestInit.body = encodedBody;
      }
      response = await this.#fetch(new Request(this.#url(path), requestInit));
    } catch {
      throw new MakoEdgeSdkError("Mako service is unavailable");
    }
    if (!response.ok) {
      const responseBody: unknown = await response.json().catch(() => null);
      if (isApiErrorEnvelope(responseBody)) {
        throw new MakoEdgeSdkError(responseBody.error.message, {
          apiError: responseBody,
          status: response.status,
        });
      }
      throw new MakoEdgeSdkError("Mako service request failed", { status: response.status });
    }
    if (response.status === 204) {
      return undefined as T;
    }
    try {
      return (await response.json()) as T;
    } catch {
      throw new MakoEdgeSdkError("Mako service returned an invalid response", {
        status: response.status,
      });
    }
  }

  #url(path: string): URL {
    return new URL(
      `v1/projects/${encodeURIComponent(this.#projectId)}/environments/${encodeURIComponent(
        this.#environmentId,
      )}/${path}`,
      this.#endpoint,
    );
  }
}

function parseEndpoint(value: string | URL): URL {
  let endpoint: URL;
  try {
    endpoint = new URL(value);
  } catch {
    throw new MakoEdgeSdkError("function SDK endpoint must be an absolute URL");
  }
  const localHttp =
    endpoint.protocol === "http:" &&
    ["localhost", "127.0.0.1", "host.docker.internal", "host.containers.internal"].includes(
      endpoint.hostname,
    );
  if (endpoint.protocol !== "https:" && !localHttp) {
    throw new MakoEdgeSdkError("function SDK endpoint must use HTTPS outside local development");
  }
  endpoint.pathname = `${endpoint.pathname.replace(/\/+$/u, "")}/`;
  endpoint.search = "";
  endpoint.hash = "";
  return endpoint;
}

function validateCallerAuthorization(value: string | null): string | null {
  if (value === null) {
    return null;
  }
  if (
    value.length < 16 ||
    value.length > 8_192 ||
    /\s/u.test(value) ||
    value.toLowerCase().startsWith("bearer")
  ) {
    throw new MakoEdgeSdkError("runtime caller authorization is invalid");
  }
  return value;
}

function validateServiceCredential(value: string): string {
  if (
    !value.startsWith("mako_sk.") ||
    value.length < 32 ||
    value.length > 512 ||
    /\s/u.test(value)
  ) {
    throw new MakoEdgeSdkError("attached service credential is invalid");
  }
  return value;
}

function validateBypassReason(value: string): string {
  if (
    value.length < 1 ||
    value.length > 512 ||
    value.trim() !== value ||
    Array.from(value).some((character) => {
      const point = character.codePointAt(0);
      return point !== undefined && (point <= 0x1f || point === 0x7f);
    })
  ) {
    throw new MakoEdgeSdkError("service bypass reason is invalid");
  }
  return value;
}

function validateRequestId(value: string | undefined): string | undefined {
  if (value !== undefined && !/^req_[A-Za-z0-9_-]{4,124}$/u.test(value)) {
    throw new MakoEdgeSdkError("runtime request identifier is invalid");
  }
  return value;
}

function validateDocumentId(value: string): void {
  if (
    value.length < 1 ||
    value.length > 1_024 ||
    value.trim() !== value ||
    Array.from(value).some((character) => {
      const point = character.codePointAt(0);
      return point !== undefined && (point <= 0x1f || point === 0x7f);
    })
  ) {
    throw new MakoEdgeSdkError("document identifier is invalid");
  }
}

function validateQuery(query: FunctionDocumentQuery): void {
  if (
    query.predicates.length < 1 ||
    query.predicates.length > 16 ||
    query.sort.length > 16 ||
    !Number.isSafeInteger(query.limit) ||
    query.limit < 1 ||
    query.limit > 1_000
  ) {
    throw new MakoEdgeSdkError("document query is invalid");
  }
}

function validateMutation<T extends JsonObject>(mutation: FunctionDocumentMutation<T>): void {
  const revisionMatchesOperation =
    (mutation.operation === "create" && mutation.expectedRevision === null) ||
    (mutation.operation !== "create" &&
      typeof mutation.expectedRevision === "string" &&
      mutation.expectedRevision.length > 0);
  if (
    mutation.mutationId.length < 16 ||
    mutation.mutationId.length > 200 ||
    !Number.isSafeInteger(mutation.schemaVersion) ||
    mutation.schemaVersion < 1 ||
    !revisionMatchesOperation ||
    typeof mutation.body !== "object" ||
    mutation.body === null ||
    Array.isArray(mutation.body)
  ) {
    throw new MakoEdgeSdkError("document mutation is invalid");
  }
}
