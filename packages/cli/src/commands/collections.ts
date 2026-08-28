import type {
  CreateCollectionRequest,
  CreateSchemaMigrationRequest,
  PublishCollectionSchemaRequest,
  SchemaMigrationState,
} from "@mako-cloud/management-sdk";

import type { CommandContext } from "../cli/context.js";
import { usageError } from "../cli/errors.js";
import type { TableColumn } from "../cli/output.js";
import type { Command, CommandArgs, OptionSpec, PositionalSpec } from "../cli/registry.js";
import { TENANT_OPTIONS, tenantFrom } from "./shared.js";

type JsonObject = Record<string, unknown>;
type PrimaryKey = CreateCollectionRequest["primaryKey"];

const MIGRATION_STATES: readonly SchemaMigrationState[] = [
  "planned",
  "running",
  "failed",
  "completed",
  "cancelled",
];

const COLLECTION_COLUMNS: readonly TableColumn[] = [
  { key: "id" },
  { key: "state" },
  { key: "schemaVersion" },
  { key: "metadataVersion" },
  { key: "compatibility" },
];

const COLLECTION_ID: PositionalSpec = {
  name: "collection-id",
  description: "Collection id",
  required: true,
};

const MIGRATION_ID: PositionalSpec = {
  name: "migration-id",
  description: "Schema migration id",
  required: true,
};

/** Options shared by `create` and `schema publish`: the schema and its primary key. */
const SCHEMA_OPTIONS: Readonly<Record<string, OptionSpec>> = {
  schema: {
    type: "string",
    required: true,
    description: "JSON schema: @path, - for stdin, or inline JSON",
    placeholder: "<@file|-|json>",
  },
  "primary-key": {
    type: "string",
    description:
      "Primary key: a field name, or a composite definition as @path or inline JSON (default: the schema's primaryKey)",
    placeholder: "<field|@file|json>",
  },
};

export function isJsonObject(value: unknown): value is JsonObject {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

async function jsonObjectFrom(
  context: CommandContext,
  source: string,
  what: string,
): Promise<JsonObject> {
  const value = await context.readJson(source, what);
  if (!isJsonObject(value)) throw usageError(`${what} must be a JSON object`);
  return value;
}

/**
 * A primary-key definition from a field name, the API's `{kind: ...}` shape, or
 * RxDB's own `primaryKey` (a string, or `{key, fields, separator}` without a kind).
 */
export function primaryKeyDefinition(value: unknown, what: string): PrimaryKey {
  if (typeof value === "string" && value !== "") return { kind: "field", field: value };
  if (isJsonObject(value)) {
    if (value.kind === "field" && typeof value.field === "string" && value.field !== "") {
      return { kind: "field", field: value.field };
    }
    if (
      (value.kind === "composite" || value.kind === undefined) &&
      typeof value.key === "string" &&
      value.key !== "" &&
      Array.isArray(value.fields) &&
      value.fields.length >= 2 &&
      value.fields.every((field): field is string => typeof field === "string" && field !== "") &&
      typeof value.separator === "string" &&
      value.separator !== ""
    ) {
      return {
        kind: "composite",
        key: value.key,
        fields: value.fields,
        separator: value.separator,
      };
    }
  }
  throw usageError(
    `${what} must be a field name, {"kind":"field","field":...}, or {"kind":"composite","key":...,"fields":[...],"separator":...}`,
  );
}

async function primaryKeyFrom(
  context: CommandContext,
  args: CommandArgs,
  schema: JsonObject,
): Promise<PrimaryKey> {
  const option = args.string("primary-key");
  if (option === undefined) {
    if (schema.primaryKey === undefined) {
      throw usageError("--primary-key is required when the schema declares no primaryKey");
    }
    return primaryKeyDefinition(schema.primaryKey, "the schema's primaryKey");
  }
  if (option.startsWith("@") || option.startsWith("{")) {
    return primaryKeyDefinition(await context.readJson(option, "--primary-key"), "--primary-key");
  }
  return primaryKeyDefinition(option, "--primary-key");
}

function schemaVersionFrom(args: CommandArgs, fallback?: number): number {
  const version = args.integer("schema-version") ?? fallback;
  if (version === undefined) throw usageError("--schema-version is required");
  if (version < 1) throw usageError("--schema-version must be a positive integer");
  return version;
}

async function list(context: CommandContext, args: CommandArgs): Promise<void> {
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  const collections = await client.listCollections(projectId, environmentId);
  context.out(collections, { columns: COLLECTION_COLUMNS });
}

async function create(context: CommandContext, args: CommandArgs): Promise<void> {
  const id = args.requirePositional(0, "collection-id");
  const { projectId, environmentId } = tenantFrom(context, args);
  const schemaVersion = schemaVersionFrom(args, 1);
  const jsonSchema = await jsonObjectFrom(context, args.requireString("schema"), "--schema");
  const primaryKey = await primaryKeyFrom(context, args, jsonSchema);
  const request: CreateCollectionRequest = { id, schemaVersion, jsonSchema, primaryKey };
  const client = await context.management();
  const collection = await client.createCollection(
    projectId,
    environmentId,
    request,
    context.idempotencyKey(),
  );
  context.out(collection);
}

async function get(context: CommandContext, args: CommandArgs): Promise<void> {
  const collectionId = args.requirePositional(0, "collection-id");
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  context.out(await client.getCollection(projectId, environmentId, collectionId));
}

async function publish(context: CommandContext, args: CommandArgs): Promise<void> {
  const collectionId = args.requirePositional(0, "collection-id");
  const { projectId, environmentId } = tenantFrom(context, args);
  const schemaVersion = schemaVersionFrom(args);
  const jsonSchema = await jsonObjectFrom(context, args.requireString("schema"), "--schema");
  const primaryKey = await primaryKeyFrom(context, args, jsonSchema);
  const request: PublishCollectionSchemaRequest = { schemaVersion, jsonSchema, primaryKey };
  const client = await context.management();
  const result = await client.publishCollectionSchema(
    projectId,
    environmentId,
    collectionId,
    request,
    context.idempotencyKey(),
  );
  if (result.status === "migration_required") {
    context.info(
      `schema version ${schemaVersion} does not fit the existing documents; plan a migration with: mako collections migrations create ${collectionId} --input <@file|-|json>`,
    );
  }
  context.out(result);
}

async function createMigration(context: CommandContext, args: CommandArgs): Promise<void> {
  const collectionId = args.requirePositional(0, "collection-id");
  const { projectId, environmentId } = tenantFrom(context, args);
  const input = await jsonObjectFrom(context, args.requireString("input"), "--input");
  const { targetSchemaVersion, targetJsonSchema, reason } = input;
  if (!Number.isInteger(targetSchemaVersion) || (targetSchemaVersion as number) < 1) {
    throw usageError("--input must carry a positive integer targetSchemaVersion");
  }
  if (!isJsonObject(targetJsonSchema)) {
    throw usageError("--input must carry a targetJsonSchema object");
  }
  if (typeof reason !== "string" || reason === "") {
    throw usageError("--input must carry a non-empty reason");
  }
  const keySource = input.targetPrimaryKey ?? targetJsonSchema.primaryKey;
  if (keySource === undefined) {
    throw usageError(
      "--input must carry targetPrimaryKey when the target schema has no primaryKey",
    );
  }
  const request: CreateSchemaMigrationRequest = {
    targetSchemaVersion: targetSchemaVersion as number,
    targetJsonSchema,
    targetPrimaryKey: primaryKeyDefinition(keySource, "targetPrimaryKey"),
    reason,
  };
  const client = await context.management();
  const migration = await client.createSchemaMigration(
    projectId,
    environmentId,
    collectionId,
    request,
    context.idempotencyKey(),
  );
  context.out(migration);
}

async function getMigration(context: CommandContext, args: CommandArgs): Promise<void> {
  const collectionId = args.requirePositional(0, "collection-id");
  const migrationId = args.requirePositional(1, "migration-id");
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  context.out(await client.getSchemaMigration(projectId, environmentId, collectionId, migrationId));
}

async function updateMigration(context: CommandContext, args: CommandArgs): Promise<void> {
  const collectionId = args.requirePositional(0, "collection-id");
  const migrationId = args.requirePositional(1, "migration-id");
  const { projectId, environmentId } = tenantFrom(context, args);
  const state = args.requireString("state");
  if (!MIGRATION_STATES.includes(state as SchemaMigrationState)) {
    throw usageError(`--state must be one of ${MIGRATION_STATES.join(", ")}`);
  }
  const client = await context.management();
  context.out(
    await client.updateSchemaMigration(
      projectId,
      environmentId,
      collectionId,
      migrationId,
      state as SchemaMigrationState,
    ),
  );
}

export const collectionsCommands: readonly Command[] = [
  {
    path: ["collections", "list"],
    summary: "List the environment's collections and their active schema versions",
    operations: ["listCollections"],
    options: { ...TENANT_OPTIONS },
    run: list,
  },
  {
    path: ["collections", "create"],
    summary: "Create a collection with its first schema",
    operations: ["createCollection"],
    positionals: [COLLECTION_ID],
    options: {
      ...TENANT_OPTIONS,
      ...SCHEMA_OPTIONS,
      "schema-version": {
        type: "string",
        description: "Schema version to record (default: 1)",
        placeholder: "<n>",
      },
    },
    run: create,
  },
  {
    path: ["collections", "get"],
    summary: "Show a collection, its active schema, and its state",
    operations: ["getCollection"],
    positionals: [COLLECTION_ID],
    options: { ...TENANT_OPTIONS },
    run: get,
  },
  {
    path: ["collections", "schema", "publish"],
    summary: "Publish a new schema version; reports migration_required when documents do not fit",
    operations: ["publishCollectionSchema"],
    positionals: [COLLECTION_ID],
    options: {
      ...TENANT_OPTIONS,
      ...SCHEMA_OPTIONS,
      "schema-version": {
        type: "string",
        required: true,
        description: "Schema version to publish",
        placeholder: "<n>",
      },
    },
    run: publish,
  },
  {
    path: ["collections", "migrations", "create"],
    summary: "Plan a schema migration to a target schema version",
    operations: ["createSchemaMigration"],
    positionals: [COLLECTION_ID],
    options: {
      ...TENANT_OPTIONS,
      input: {
        type: "string",
        required: true,
        description:
          "Migration JSON {targetSchemaVersion, targetJsonSchema, targetPrimaryKey?, reason}: @path, - for stdin, or inline",
        placeholder: "<@file|-|json>",
      },
    },
    run: createMigration,
  },
  {
    path: ["collections", "migrations", "get"],
    summary: "Show a schema migration and its state",
    operations: ["getSchemaMigration"],
    positionals: [COLLECTION_ID, MIGRATION_ID],
    options: { ...TENANT_OPTIONS },
    run: getMigration,
  },
  {
    path: ["collections", "migrations", "update"],
    summary: "Move a schema migration to another state",
    operations: ["updateSchemaMigration"],
    positionals: [COLLECTION_ID, MIGRATION_ID],
    options: {
      ...TENANT_OPTIONS,
      state: {
        type: "string",
        required: true,
        description: `New state: ${MIGRATION_STATES.join(", ")}`,
        placeholder: "<state>",
      },
    },
    run: updateMigration,
  },
];
