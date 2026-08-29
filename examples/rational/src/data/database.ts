import { createRxDatabase, removeRxDatabase, type RxCollection, type RxDatabase } from "rxdb";
import { getRxStorageDexie } from "rxdb/plugins/storage-dexie";

import { rxdbSchema } from "../model/collections.js";
import type { CollectionId, RationalDocuments } from "../model/types.js";
import { conflictHandler } from "./conflict.js";

export type RationalCollections<Ids extends CollectionId> = {
  [Id in Ids]: RxCollection<RationalDocuments[Id]>;
};

export type RationalDatabase<Ids extends CollectionId> = RxDatabase<RationalCollections<Ids>>;

/**
 * An RxDB database on Dexie (IndexedDB) with one collection per document
 * type, every collection sharing the deterministic conflict handler. The
 * storage is durable, which is the point: a reload or an offline restart
 * finds the household's data where it left it.
 */
export async function openDatabase<Ids extends CollectionId>(
  name: string,
  collectionIds: readonly Ids[],
): Promise<RationalDatabase<Ids>> {
  const database = await createRxDatabase<RationalCollections<Ids>>({
    name,
    storage: getRxStorageDexie(),
    multiInstance: false,
    eventReduce: true,
  });
  const definitions = Object.fromEntries(
    collectionIds.map((id) => [id, { schema: rxdbSchema(id), conflictHandler: conflictHandler() }]),
  );
  // RxDB types the creators against every collection of the database type;
  // this database holds exactly the ids it was asked for.
  await database.addCollections(definitions as never);
  return database;
}

/**
 * Erase a database that is not open. A security reset that arrives while the
 * scope holds no session — the epochs moved between two visits — still has to
 * clear what the previous generation left on the device.
 */
export async function removeDatabase(name: string): Promise<void> {
  await removeRxDatabase(name, getRxStorageDexie());
}
