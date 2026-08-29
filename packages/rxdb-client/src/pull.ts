import type {
  ReplicationPullHandler,
  ReplicationPullOptions,
  RxReplicationPullStreamItem,
  WithDeleted,
} from "rxdb";
import type { Observable } from "rxjs";

import type { MakoAuthClient } from "./auth.js";
import type { NormalizedMakoRxdbClientConfig } from "./config.js";
import { replicationNetworkError } from "./replication-error.js";
import { replicationAccessToken, sendReplicationRequest } from "./replication-session.js";
import type { ReplicationCheckpointPersistence } from "./replication-state.js";
import type { MakoReplicationDocument } from "./wire.js";

/** RxDB checkpoints must be mergeable objects; the service token stays opaque. */
export interface MakoCheckpoint {
  readonly token: string;
}

export interface MakoPullAdapterOptions<RxDocType = unknown> {
  readonly fetch?: typeof globalThis.fetch;
  readonly stream$?: Observable<RxReplicationPullStreamItem<RxDocType, MakoCheckpoint>>;
  /** Records every checkpoint a pull returns so a restart can resume the live stream from it. */
  readonly checkpoints?: ReplicationCheckpointPersistence;
}

export function createMakoPullHandler<RxDocType>(
  config: NormalizedMakoRxdbClientConfig,
  auth: MakoAuthClient,
  options: MakoPullAdapterOptions<RxDocType> = {},
): ReplicationPullHandler<RxDocType, MakoCheckpoint> {
  const fetch = options.fetch ?? globalThis.fetch;
  const checkpoints = options.checkpoints;
  return async (checkpoint, _rxdbBatchSize) => {
    const accessToken = await replicationAccessToken(auth);
    const response = await sendReplicationRequest(auth, accessToken, async (token) => {
      try {
        return await fetch(replicationUrl(config, "pull"), {
          method: "POST",
          headers: {
            Accept: "application/json",
            Authorization: `Bearer ${token}`,
            "Content-Type": "application/json",
            "X-Mako-Key": config.publicProjectKey,
          },
          body: JSON.stringify({
            checkpoint: checkpoint?.token ?? null,
            schemaVersion: config.schemaVersion,
            batchSize: config.pullBatchSize,
          }),
        });
      } catch {
        throw replicationNetworkError();
      }
    });
    const body: unknown = await response.json();
    if (!isPullResponse(body)) {
      throw replicationNetworkError();
    }
    const nextCheckpoint: MakoCheckpoint = { token: body.checkpoint };
    await checkpoints?.save(nextCheckpoint);
    return {
      documents: body.documents as WithDeleted<RxDocType>[],
      checkpoint: nextCheckpoint,
    };
  };
}

export function createMakoPullOptions<RxDocType>(
  config: NormalizedMakoRxdbClientConfig,
  auth: MakoAuthClient,
  options: MakoPullAdapterOptions<RxDocType> = {},
): ReplicationPullOptions<RxDocType, MakoCheckpoint> {
  const pull: ReplicationPullOptions<RxDocType, MakoCheckpoint> = {
    handler: createMakoPullHandler(config, auth, options),
    batchSize: config.pullBatchSize,
  };
  if (options.stream$ !== undefined) {
    pull.stream$ = options.stream$;
  }
  return pull;
}

function replicationUrl(config: NormalizedMakoRxdbClientConfig, route: string): URL {
  const base = new URL(config.endpoint);
  base.pathname = `${base.pathname.replace(/\/+$/u, "")}/`;
  return new URL(
    `v1/projects/${encodeURIComponent(config.projectId)}/environments/${encodeURIComponent(
      config.environmentId,
    )}/collections/${encodeURIComponent(config.collectionId)}/replication/${route}`,
    base,
  );
}

function isPullResponse(
  value: unknown,
): value is { documents: MakoReplicationDocument[]; checkpoint: string } {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const candidate = value as { documents?: unknown; checkpoint?: unknown };
  return (
    Array.isArray(candidate.documents) &&
    candidate.documents.every(
      (document) =>
        typeof document === "object" &&
        document !== null &&
        typeof (document as { _deleted?: unknown })._deleted === "boolean",
    ) &&
    typeof candidate.checkpoint === "string"
  );
}
