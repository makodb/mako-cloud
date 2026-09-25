import type { MakoAuthClient } from "@mako-cloud/rxdb";
import type {
  ReferenceBackend,
  ReferenceBackendConfig,
  ReferenceBackendDiagnostics,
} from "./backend.js";
import type { ReferenceTodo } from "./reference-app.js";

type WireTodo = ReferenceTodo & { _deleted: boolean };

export interface FakeBackendDiagnostics {
  readonly acceptedWrites: number;
  readonly conflictResponses: number;
  readonly online: boolean;
  readonly refreshes: number;
  readonly streamConnections: number;
}

/**
 * A deterministic, in-browser implementation of the public Mako auth and
 * replication protocol. It exists only to make the reference app runnable and
 * its browser tests hermetic; production applications point the same adapters
 * at a hosted Mako endpoint.
 */
export class FakeMakoBackend implements ReferenceBackend {
  readonly #documents = new Map<string, WireTodo>();
  readonly #changes: Array<{ sequence: number; document: WireTodo }> = [];
  readonly #streams = new Set<ReadableStreamDefaultController<Uint8Array>>();
  #acceptedWrites = 0;
  #clock = 1_800_000_000_000;
  #conflictResponses = 0;
  #online = true;
  #refreshes = 0;
  #revoked = false;
  #requiredSchemaVersion: number | null = null;
  #sequence = 0;
  #streamConnections = 0;

  readonly config: ReferenceBackendConfig = {
    endpoint: "http://127.0.0.1:4173/",
    projectId: "prj_example01",
    environmentId: "env_example01",
    collectionId: "todos",
    publicProjectKey: "mako_pk.reference-app",
  };

  readonly now = (): number => this.#clock;

  async authenticate(auth: MakoAuthClient): Promise<void> {
    await auth.signInWithPassword("local@example.test", "reference-password");
  }

  /** The fake ages its own clock rather than exchanging a real credential. */
  async forceTokenRefresh(auth: MakoAuthClient): Promise<void> {
    this.advanceClock(40_000);
    await auth.validAccessToken();
  }

  readonly fetch: typeof globalThis.fetch = async (input, init = {}) => {
    const url = requestUrl(input);
    if (url.pathname.endsWith("/auth/signin")) {
      return jsonResponse(this.#session("access-initial", "refresh-initial", 60));
    }
    if (url.pathname.endsWith("/auth/token")) {
      this.#refreshes += 1;
      if (this.#revoked) {
        return apiError(401, "unauthenticated", "the session was revoked", "never");
      }
      return jsonResponse(this.#session(`access-${this.#refreshes}`, "refresh-current", 3_600));
    }
    if (url.pathname.endsWith("/auth/signout")) {
      return new Response(null, { status: 204 });
    }
    if (!url.pathname.includes("/replication/")) {
      return apiError(404, "not_found", "route not found", "never");
    }
    if (!this.#online) {
      throw new TypeError("simulated offline network");
    }
    if (this.#revoked) {
      return apiError(401, "unauthenticated", "the session was revoked", "never");
    }
    if (this.#requiredSchemaVersion !== null) {
      return schemaMismatch(this.#requiredSchemaVersion);
    }
    if (url.pathname.endsWith("/replication/pull")) {
      return this.#pull(init);
    }
    if (url.pathname.endsWith("/replication/push")) {
      return this.#push(init);
    }
    if (url.pathname.endsWith("/replication/stream")) {
      return this.#stream(init.signal);
    }
    return apiError(404, "not_found", "route not found", "never");
  };

  diagnostics(): ReferenceBackendDiagnostics {
    return {
      acceptedWrites: this.#acceptedWrites,
      conflictResponses: this.#conflictResponses,
      online: this.#online,
      refreshes: this.#refreshes,
      streamConnections: this.#streamConnections,
    };
  }

  advanceClock(milliseconds: number): void {
    this.#clock += milliseconds;
  }

  setOnline(online: boolean): void {
    this.#online = online;
    if (!online) {
      this.disconnectStreams();
    }
  }

  requireSchemaVersion(version: number): void {
    this.#requiredSchemaVersion = version;
    this.disconnectStreams();
  }

  async revokeAccess(_auth?: MakoAuthClient): Promise<void> {
    this.#revoked = true;
    this.#clock += 4_000_000;
    this.disconnectStreams();
  }

  async putRemote(document: ReferenceTodo): Promise<void> {
    this.#record({ ...structuredClone(document), _deleted: false });
  }

  async deleteRemote(id: string, updatedAt: number): Promise<void> {
    const current = this.#documents.get(id);
    this.#record({
      id,
      ownerId: current?.ownerId ?? "user-example",
      title: current?.title ?? "deleted",
      updatedAt,
      _deleted: true,
    });
  }

  disconnectStreams(): void {
    for (const stream of this.#streams) {
      stream.error(new TypeError("simulated stream disconnect"));
    }
    this.#streams.clear();
  }

  #session(accessToken: string, refreshToken: string, expiresIn: number): object {
    return {
      accessToken,
      refreshToken,
      expiresIn,
      user: {
        id: "user-example",
        email: "local@example.test",
        status: "active",
        authorizationEpoch: this.#revoked ? 2 : 1,
      },
    };
  }

  async #pull(init: RequestInit): Promise<Response> {
    const body = await jsonBody<{ checkpoint?: string | null }>(init);
    const sequence = parseCheckpoint(body.checkpoint ?? null);
    return jsonResponse({
      documents: this.#changes
        .filter((change) => change.sequence > sequence)
        .map((change) => structuredClone(change.document)),
      checkpoint: checkpoint(this.#sequence),
    });
  }

  async #push(init: RequestInit): Promise<Response> {
    const body = await jsonBody<{
      rows: Array<{
        mutationId: string;
        assumedMasterState: WireTodo | null;
        newDocumentState: WireTodo;
      }>;
    }>(init);
    const outcomes = body.rows.map((row) => {
      const current = this.#documents.get(row.newDocumentState.id);
      const assumed = row.assumedMasterState ?? undefined;
      if (!sameDocument(current, assumed)) {
        this.#conflictResponses += 1;
        return {
          mutationId: row.mutationId,
          status: "conflict",
          masterState: current ?? ({ ...row.newDocumentState, _deleted: true } satisfies WireTodo),
        };
      }
      this.#acceptedWrites += 1;
      this.#record(structuredClone(row.newDocumentState));
      return { mutationId: row.mutationId, status: "accepted" };
    });
    return jsonResponse({ outcomes });
  }

  #stream(signal?: AbortSignal | null): Response {
    this.#streamConnections += 1;
    const encoder = new TextEncoder();
    const streams = this.#streams;
    let activeController: ReadableStreamDefaultController<Uint8Array> | undefined;
    const body = new ReadableStream<Uint8Array>({
      start(controller) {
        activeController = controller;
        streams.add(controller);
        controller.enqueue(
          encoder.encode(
            `data: ${JSON.stringify({
              event: "heartbeat",
              data: { cursor: `msc1.${Date.now().toString().padStart(16, "0")}` },
            })}\n\n`,
          ),
        );
        signal?.addEventListener(
          "abort",
          () => {
            if (streams.delete(controller)) {
              controller.error(new DOMException("stream aborted", "AbortError"));
            }
          },
          { once: true },
        );
      },
      cancel() {
        if (activeController !== undefined) {
          streams.delete(activeController);
        }
      },
    });
    return new Response(body, {
      headers: { "Content-Type": "text/event-stream" },
      status: 200,
    });
  }

  #record(document: WireTodo): void {
    this.#sequence += 1;
    const snapshot = structuredClone(document);
    this.#documents.set(snapshot.id, snapshot);
    this.#changes.push({ sequence: this.#sequence, document: snapshot });
    const frame = new TextEncoder().encode(
      `data: ${JSON.stringify({
        event: "documents",
        data: {
          documents: [snapshot],
          checkpoint: checkpoint(this.#sequence),
          cursor: `msc1.${this.#sequence.toString().padStart(16, "0")}`,
        },
      })}\n\n`,
    );
    for (const stream of this.#streams) {
      stream.enqueue(frame);
    }
  }
}

function requestUrl(input: RequestInfo | URL): URL {
  if (typeof input === "string") {
    return new URL(input);
  }
  if (input instanceof URL) {
    return input;
  }
  return new URL(input.url);
}

async function jsonBody<T>(init: RequestInit): Promise<T> {
  if (typeof init.body !== "string") {
    throw new TypeError("expected a JSON request body");
  }
  return JSON.parse(init.body) as T;
}

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

function apiError(
  status: number,
  code: string,
  message: string,
  retryKind: "never" | "immediate",
): Response {
  return jsonResponse(
    {
      error: {
        code,
        message,
        requestId: "req_reference_app",
        retry: { kind: retryKind },
      },
    },
    status,
  );
}

/** The answer of a server whose collection's active schema is now `version`. */
function schemaMismatch(version: number): Response {
  return jsonResponse(
    {
      apiVersion: "v1",
      error: {
        code: "schema_mismatch",
        message: "replication schema migration is required",
        requestId: "req_reference_app",
        retry: { kind: "never" },
        details: { requiredSchemaVersion: String(version) },
      },
    },
    409,
  );
}

function checkpoint(sequence: number): string {
  return `mcp1.${sequence.toString().padStart(16, "0")}`;
}

function parseCheckpoint(value: string | null): number {
  if (value === null) {
    return 0;
  }
  const sequence = Number(value.slice(value.lastIndexOf(".") + 1));
  return Number.isSafeInteger(sequence) && sequence >= 0 ? sequence : 0;
}

function sameDocument(left: WireTodo | undefined, right: WireTodo | undefined): boolean {
  if (left === undefined || right === undefined) {
    return left === right;
  }
  return (
    left.id === right.id &&
    left.ownerId === right.ownerId &&
    left.title === right.title &&
    left.updatedAt === right.updatedAt &&
    left._deleted === right._deleted
  );
}
