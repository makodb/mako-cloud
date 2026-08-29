import {
  createMakoPullHandler,
  createMakoPushOptions,
  type MakoCheckpoint,
  MakoLivePullStream,
  MakoReplicationSignals,
  type MakoResyncReason,
  type NormalizedMakoRxdbClientConfig,
  normalizeMakoRxdbConfig,
  type ReplicationCheckpointPersistence,
} from "@mako-cloud/rxdb";
import type { ReplicationPullHandler, RxCollection, RxReplicationPullStreamItem } from "rxdb";
import { type RxReplicationState, replicateRxCollection } from "rxdb/plugins/replication";
import { RXDB_VERSION } from "rxdb/plugins/utils";
import { map } from "rxjs";
import type { Subscription } from "rxjs";

import type { RationalAuth } from "../auth.js";
import type { RationalConfig } from "../config.js";
import { SCHEMA_VERSION } from "../model/collections.js";
import type { BaseDocument, CollectionId, RationalDocuments } from "../model/types.js";
import type { Transport } from "./transport.js";

export interface ReplicationDependencies {
  readonly config: RationalConfig;
  readonly auth: RationalAuth;
  readonly transport: Transport;
}

export interface CollectionReplicationOptions {
  /** Base replication identifier; the collection id is appended. */
  readonly identifier: string;
  /** Keep only documents of this household; the server cannot filter a pull. */
  readonly householdId?: string;
  /** Open a live server-sent-events stream for this collection. */
  readonly stream: boolean;
  /** Where every checkpoint is recorded so a restart resumes from it. */
  readonly checkpoints?: ReplicationCheckpointPersistence;
  /** The checkpoint a previous run reached; the stream resumes there. */
  readonly initialCheckpoint?: MakoCheckpoint;
  readonly onResync: (reason: MakoResyncReason) => void | Promise<void>;
  readonly onError: (error: unknown) => void | Promise<void>;
}

export interface CollectionReplication<T> {
  readonly collectionId: CollectionId;
  readonly replication: RxReplicationState<T, MakoCheckpoint>;
  readonly signals: MakoReplicationSignals<T>;
  readonly live: MakoLivePullStream<T> | null;
  readonly subscription: Subscription;
  stop(): Promise<void>;
}

export function makoConfigFor(
  config: RationalConfig,
  collectionId: CollectionId,
): NormalizedMakoRxdbClientConfig {
  return normalizeMakoRxdbConfig({
    endpoint: config.endpoint,
    projectId: config.projectId,
    environmentId: config.environmentId,
    collectionId,
    schemaVersion: SCHEMA_VERSION,
    publicProjectKey: config.publicProjectKey,
    rxdbVersion: RXDB_VERSION,
    runtime: "browser",
    pullBatchSize: 100,
    pushBatchSize: 100,
  });
}

/**
 * Wire one RxDB collection to its Mako collection exactly as the reference
 * application does — the package's pull, push, and live adapters, RxDB's
 * replication state, and the package's signals on top — with two additions:
 * the transport in the middle, and a pull filter for the household when the
 * database holds one household only.
 */
export function startCollectionReplication<Id extends CollectionId>(
  collection: RxCollection<RationalDocuments[Id]>,
  collectionId: Id,
  dependencies: ReplicationDependencies,
  options: CollectionReplicationOptions,
): CollectionReplication<RationalDocuments[Id]> {
  type T = RationalDocuments[Id];
  const { auth, transport, config } = dependencies;
  const makoConfig = makoConfigFor(config, collectionId);
  const fetch = transport.fetch;
  const belongs = (document: BaseDocument) =>
    options.householdId === undefined || document.household_id === options.householdId;

  const checkpoints = options.checkpoints;
  const live =
    options.stream === false
      ? null
      : new MakoLivePullStream<T>(makoConfig, auth.client, {
          fetch,
          ...(checkpoints === undefined ? {} : { checkpoints }),
          reconnectMinimumDelayMs: Math.max(10, Math.min(config.retryTimeMs, 60_000)),
          reconnectMaximumDelayMs: Math.max(40, Math.min(config.retryTimeMs * 8, 300_000)),
          onResyncReason: options.onResync,
        });
  const stream$ = live?.stream$.pipe(
    map(
      (event): RxReplicationPullStreamItem<T, MakoCheckpoint> =>
        event === "RESYNC"
          ? event
          : { ...event, documents: event.documents.filter((document) => belongs(document)) },
    ),
  );

  const inner = createMakoPullHandler<T>(makoConfig, auth.client, {
    fetch,
    ...(checkpoints === undefined ? {} : { checkpoints }),
  });
  // A pull returns every document the user may read, across all their
  // households. Documents of other households are dropped here; when a whole
  // server page is dropped the handler keeps pulling so RxDB still sees
  // progress instead of an empty batch it would not advance past.
  const handler: ReplicationPullHandler<T, MakoCheckpoint> = async (checkpoint, batchSize) => {
    let current = checkpoint;
    const collected: Awaited<ReturnType<typeof inner>>["documents"] = [];
    for (let page = 0; page < 50; page += 1) {
      const result = await inner(current, batchSize);
      current = result.checkpoint;
      collected.push(...result.documents.filter((document) => belongs(document)));
      if (
        result.documents.length < makoConfig.pullBatchSize ||
        collected.length >= makoConfig.pullBatchSize
      ) {
        break;
      }
    }
    return { documents: collected, checkpoint: current };
  };

  const replication = replicateRxCollection<T, MakoCheckpoint>({
    replicationIdentifier: `${options.identifier}:${collectionId}`,
    collection,
    pull: {
      handler,
      batchSize: makoConfig.pullBatchSize,
      ...(stream$ === undefined ? {} : { stream$ }),
    },
    push: createMakoPushOptions<T>(makoConfig, auth.client, { fetch }),
    live: true,
    retryTime: config.retryTimeMs,
    waitForLeadership: false,
    toggleOnDocumentVisible: false,
    autoStart: true,
  });
  const signals = new MakoReplicationSignals<T>();
  const subscription = signals.bind(replication);
  subscription.add(replication.error$.subscribe((error) => void options.onError(error)));
  live?.start(options.initialCheckpoint);

  return {
    collectionId,
    replication,
    signals,
    live,
    subscription,
    async stop() {
      subscription.unsubscribe();
      signals.complete();
      live?.close();
      await replication.cancel();
    },
  };
}
