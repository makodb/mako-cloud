import {
  createMakoPullHandler,
  createMakoPushOptions,
  type MakoCheckpoint,
  MakoReplicationSignals,
  type MakoResyncReason,
  type NormalizedMakoRxdbClientConfig,
  normalizeMakoRxdbConfig,
  type ReplicationCheckpointPersistence,
} from "@mako-cloud/rxdb";
import type { RxCollection, RxReplicationPullStreamItem } from "rxdb";
import { type RxReplicationState, replicateRxCollection } from "rxdb/plugins/replication";
import { RXDB_VERSION } from "rxdb/plugins/utils";
import type { Observable, Subscription } from "rxjs";

import type { RationalAuth } from "../auth.js";
import type { RationalConfig } from "../config.js";
import { SCHEMA_VERSION } from "../model/collections.js";
import type { CollectionId, RationalDocuments } from "../model/types.js";
import type { Transport } from "./transport.js";

export interface ReplicationDependencies {
  readonly config: RationalConfig;
  readonly auth: RationalAuth;
  readonly transport: Transport;
}

export interface CollectionReplicationOptions {
  /** Base replication identifier; the collection id is appended. */
  readonly identifier: string;
  /** The household this database holds; the server narrows pull and stream to it. */
  readonly householdId?: string;
  /** This collection's share of the scope's one live stream, when it streams. */
  readonly stream$?: Observable<RxReplicationPullStreamItem<never, MakoCheckpoint>>;
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
  readonly subscription: Subscription;
  stop(): Promise<void>;
}

export function makoConfigFor(
  config: RationalConfig,
  collectionId: CollectionId,
  householdId?: string,
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
    // One household per database, so one household per replication. The
    // server narrows the pull and the stream to it, which is why this
    // database never sees another household's documents and never has to
    // discard a page of them. The directory scope passes no household,
    // because it is not one household's.
    ...(householdId === undefined ? {} : { filter: { field: "household_id", value: householdId } }),
  });
}

/**
 * Wire one RxDB collection to its Mako collection exactly as the reference
 * application does — the package's pull and push adapters, one live stream
 * shared by every collection, RxDB's replication state, and the package's
 * signals on top — with the transport in the middle.
 */
export function startCollectionReplication<Id extends CollectionId>(
  collection: RxCollection<RationalDocuments[Id]>,
  collectionId: Id,
  dependencies: ReplicationDependencies,
  options: CollectionReplicationOptions,
): CollectionReplication<RationalDocuments[Id]> {
  type T = RationalDocuments[Id];
  const { auth, transport, config } = dependencies;
  const makoConfig = makoConfigFor(config, collectionId, options.householdId);
  const fetch = transport.fetch;
  const checkpoints = options.checkpoints;
  const stream$ = options.stream$ as
    | Observable<RxReplicationPullStreamItem<T, MakoCheckpoint>>
    | undefined;
  const handler = createMakoPullHandler<T>(makoConfig, auth.client, {
    fetch,
    ...(checkpoints === undefined ? {} : { checkpoints }),
  });

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

  return {
    collectionId,
    replication,
    signals,
    subscription,
    async stop() {
      subscription.unsubscribe();
      signals.complete();
      await replication.cancel();
    },
  };
}
