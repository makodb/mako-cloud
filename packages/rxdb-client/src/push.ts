import { isApiErrorEnvelope } from "@mako-cloud/api-types";
import type {
  ReplicationPushHandler,
  ReplicationPushOptions,
  RxReplicationWriteToMasterRow,
  WithDeleted,
} from "rxdb";

import type { MakoAuthClient } from "./auth.js";
import type { NormalizedMakoRxdbClientConfig } from "./config.js";
import {
  MakoReplicationError,
  authenticationRequiredError,
  replicationNetworkError,
  replicationResponseError,
} from "./replication-error.js";

export interface MakoPushAdapterOptions {
  readonly fetch?: typeof globalThis.fetch;
}

export function createMakoPushHandler<RxDocType>(
  config: NormalizedMakoRxdbClientConfig,
  auth: MakoAuthClient,
  options: MakoPushAdapterOptions = {},
): ReplicationPushHandler<RxDocType> {
  const fetch = options.fetch ?? globalThis.fetch;
  return async (rows) => {
    if (rows.length < 1 || rows.length > config.pushBatchSize) {
      throw clientError("invalid_request", "push batch exceeds the configured limit");
    }
    const accessToken = await auth.validAccessToken().catch(() => {
      throw authenticationRequiredError();
    });
    const wireRows = await Promise.all(
      rows.map(async (row) => ({
        mutationId: await mutationId(row),
        assumedMasterState: row.assumedMasterState ?? null,
        newDocumentState: row.newDocumentState,
      })),
    );
    const idempotencyKey = await digest(
      canonicalJson(wireRows.map((row) => row.mutationId)),
      "batch",
    );
    let response: Response;
    try {
      response = await fetch(replicationUrl(config), {
        method: "POST",
        headers: {
          Accept: "application/json",
          Authorization: `Bearer ${accessToken}`,
          "Content-Type": "application/json",
          "Idempotency-Key": idempotencyKey,
          "X-Mako-Key": config.publicProjectKey,
        },
        body: JSON.stringify({ schemaVersion: config.schemaVersion, rows: wireRows }),
      });
    } catch {
      throw replicationNetworkError();
    }
    if (!response.ok) {
      throw await replicationResponseError(response);
    }
    const body: unknown = await response.json();
    if (!isPushResponse(body) || body.outcomes.length !== rows.length) {
      throw replicationNetworkError();
    }
    const expectedIds = new Set(wireRows.map((row) => row.mutationId));
    const conflicts: WithDeleted<RxDocType>[] = [];
    for (const outcome of body.outcomes) {
      if (!expectedIds.delete(outcome.mutationId)) {
        throw replicationNetworkError();
      }
      if (outcome.status === "conflict") {
        if (!isDeletedDocument(outcome.masterState)) {
          throw replicationNetworkError();
        }
        conflicts.push(outcome.masterState as WithDeleted<RxDocType>);
      } else if (outcome.status === "denied") {
        if (!isApiErrorEnvelope(outcome.error)) {
          throw clientError("permission_denied", "document mutation is not permitted");
        }
        throw new MakoReplicationError(outcome.error.error);
      }
    }
    return conflicts;
  };
}

export function createMakoPushOptions<RxDocType>(
  config: NormalizedMakoRxdbClientConfig,
  auth: MakoAuthClient,
  options: MakoPushAdapterOptions = {},
): ReplicationPushOptions<RxDocType> {
  return {
    handler: createMakoPushHandler(config, auth, options),
    batchSize: config.pushBatchSize,
  };
}

async function mutationId<RxDocType>(
  row: RxReplicationWriteToMasterRow<RxDocType>,
): Promise<string> {
  return digest(
    canonicalJson({
      assumedMasterState: row.assumedMasterState ?? null,
      newDocumentState: row.newDocumentState,
    }),
    "mutation",
  );
}

async function digest(value: string, domain: string): Promise<string> {
  const bytes = new TextEncoder().encode(`mako-rxdb-${domain}-v1\0${value}`);
  const hash = new Uint8Array(await globalThis.crypto.subtle.digest("SHA-256", bytes));
  return `m1_${Array.from(hash, (byte) => byte.toString(16).padStart(2, "0")).join("")}`;
}

function canonicalJson(value: unknown): string {
  if (Array.isArray(value)) {
    return `[${value.map(canonicalJson).join(",")}]`;
  }
  if (value !== null && typeof value === "object") {
    return `{${Object.entries(value)
      .filter(([, child]) => child !== undefined)
      .sort(([left], [right]) => left.localeCompare(right))
      .map(([key, child]) => `${JSON.stringify(key)}:${canonicalJson(child)}`)
      .join(",")}}`;
  }
  const encoded = JSON.stringify(value);
  if (encoded === undefined) {
    throw clientError("invalid_request", "pushed state is not JSON serializable");
  }
  return encoded;
}

function replicationUrl(config: NormalizedMakoRxdbClientConfig): URL {
  const base = new URL(config.endpoint);
  base.pathname = `${base.pathname.replace(/\/+$/u, "")}/`;
  return new URL(
    `v1/projects/${encodeURIComponent(config.projectId)}/environments/${encodeURIComponent(
      config.environmentId,
    )}/collections/${encodeURIComponent(config.collectionId)}/replication/push`,
    base,
  );
}

function isPushResponse(value: unknown): value is {
  outcomes: Array<{
    mutationId: string;
    status: "accepted" | "conflict" | "denied";
    masterState?: unknown;
    error?: unknown;
  }>;
} {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const outcomes = (value as { outcomes?: unknown }).outcomes;
  return (
    Array.isArray(outcomes) &&
    outcomes.every(
      (outcome) =>
        typeof outcome === "object" &&
        outcome !== null &&
        typeof (outcome as { mutationId?: unknown }).mutationId === "string" &&
        ["accepted", "conflict", "denied"].includes(
          String((outcome as { status?: unknown }).status),
        ),
    )
  );
}

function isDeletedDocument(
  value: unknown,
): value is Record<string, unknown> & { _deleted: boolean } {
  return (
    typeof value === "object" &&
    value !== null &&
    typeof (value as { _deleted?: unknown })._deleted === "boolean"
  );
}

function clientError(
  code: "invalid_request" | "permission_denied",
  message: string,
): MakoReplicationError {
  return new MakoReplicationError({
    code,
    message,
    requestId: "req_client",
    retry: { kind: "never" },
  });
}
