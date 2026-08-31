import type { RxJsonSchema } from "rxdb";

import model from "../../mako/collections.json" with { type: "json" };
import {
  type CollectionId,
  DIRECTORY_COLLECTIONS,
  HOUSEHOLD_COLLECTIONS,
  type RationalDocuments,
} from "./types.js";

/**
 * One source, two derived forms. `mako/collections.json` is what the bootstrap
 * publishes to the platform (jsonSchema, primaryKey, indexes); this module
 * derives the RxDB schema for each collection from the very same object, so
 * the local and the server-side schema cannot drift apart.
 */
export interface PlatformIndex {
  readonly name: string;
  readonly fields: readonly string[];
}

export interface CollectionDefinition {
  readonly id: CollectionId;
  readonly scope: "directory" | "household";
  readonly jsonSchema: Record<string, unknown>;
  readonly primaryKey: { readonly kind: "field"; readonly field: string };
  readonly indexes: readonly PlatformIndex[];
}

/** The platform schema version every replication request names. */
export const SCHEMA_VERSION: number = model.schemaVersion;

/**
 * The model also holds collections the app never opens -- `plaid_items` is a
 * server-only credential store whose policy allows no application user
 * anything -- so only the collections the app actually speaks are derived.
 */
const OPENED: readonly string[] = [...DIRECTORY_COLLECTIONS, ...HOUSEHOLD_COLLECTIONS];

export const COLLECTION_DEFINITIONS: readonly CollectionDefinition[] = model.collections
  .filter((collection) => OPENED.includes(collection.id))
  .map((collection) => ({
    id: collection.id as CollectionId,
    scope: collection.scope as "directory" | "household",
    jsonSchema: collection.jsonSchema as Record<string, unknown>,
    primaryKey: collection.primaryKey as { kind: "field"; field: string },
    indexes: collection.indexes,
  }));

export function collectionDefinition(id: CollectionId): CollectionDefinition {
  const definition = COLLECTION_DEFINITIONS.find((candidate) => candidate.id === id);
  if (definition === undefined) {
    throw new Error(`collection ${id} is not part of the Rational model`);
  }
  return definition;
}

/**
 * The RxDB schema for a collection: the platform JSON schema plus RxDB's own
 * envelope (a local schema version, the primary key, and compound indexes
 * matching the platform's).
 */
export function rxdbSchema<Id extends CollectionId>(id: Id): RxJsonSchema<RationalDocuments[Id]> {
  const definition = collectionDefinition(id);
  const { properties, required } = definition.jsonSchema as {
    properties: Record<string, unknown>;
    required: string[];
  };
  const schema = {
    version: 0,
    primaryKey: definition.primaryKey.field,
    type: "object",
    properties,
    required,
    indexes: definition.indexes.map((index) => [...index.fields]),
  };
  return schema as unknown as RxJsonSchema<RationalDocuments[Id]>;
}
