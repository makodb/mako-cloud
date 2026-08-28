import type { CreateCollectionIndexRequest } from "@mako-cloud/management-sdk";

import type { CommandContext } from "../cli/context.js";
import { usageError } from "../cli/errors.js";
import type { TableColumn } from "../cli/output.js";
import type { Command, CommandArgs, PositionalSpec } from "../cli/registry.js";
import { TENANT_OPTIONS, tenantFrom } from "./shared.js";

type IndexField = CreateCollectionIndexRequest["fields"][number];

const INDEX_COLUMNS: readonly TableColumn[] = [
  { key: "name" },
  { key: "version" },
  { key: "kind" },
  { key: "state" },
  { key: "activationFenced", label: "fenced" },
];

const COLLECTION_ID: PositionalSpec = {
  name: "collection-id",
  description: "Collection id",
  required: true,
};

const INDEX_NAME: PositionalSpec = { name: "name", description: "Index name", required: true };

const INDEX_VERSION: PositionalSpec = {
  name: "version",
  description: "Index version",
  required: true,
};

/** A non-negative integer positional, such as an index version. */
export function integerPositional(args: CommandArgs, index: number, name: string): number {
  const raw = args.requirePositional(index, name);
  if (!/^\d+$/u.test(raw)) throw usageError(`<${name}> must be a non-negative integer`);
  return Number.parseInt(raw, 10);
}

/** `path`, `path:ascending`, or `path:descending` (`asc`/`desc` accepted). */
function indexFieldFrom(spec: string): IndexField {
  const separator = spec.lastIndexOf(":");
  const path = separator === -1 ? spec : spec.slice(0, separator);
  const direction = separator === -1 ? "ascending" : spec.slice(separator + 1);
  if (path === "") throw usageError(`--field needs a field path, got "${spec}"`);
  if (direction === "ascending" || direction === "asc") return { path, direction: "ascending" };
  if (direction === "descending" || direction === "desc") return { path, direction: "descending" };
  throw usageError(`--field direction must be ascending or descending, got "${spec}"`);
}

async function list(context: CommandContext, args: CommandArgs): Promise<void> {
  const collectionId = args.requirePositional(0, "collection-id");
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  const indexes = await client.listCollectionIndexes(projectId, environmentId, collectionId);
  context.out(indexes, { columns: INDEX_COLUMNS });
}

async function create(context: CommandContext, args: CommandArgs): Promise<void> {
  const collectionId = args.requirePositional(0, "collection-id");
  const { projectId, environmentId } = tenantFrom(context, args);
  const name = args.requireString("name");
  const version = args.integer("version");
  if (version === undefined || version < 1) {
    throw usageError("--version must be a positive integer");
  }
  const fields = args.strings("field").map(indexFieldFrom);
  if (fields.length === 0) throw usageError("at least one --field <path[:direction]> is required");
  const request: CreateCollectionIndexRequest = {
    name,
    version,
    kind: args.boolean("unique") ? "unique" : "non_unique",
    fields,
  };
  const client = await context.management();
  const index = await client.createCollectionIndex(
    projectId,
    environmentId,
    collectionId,
    request,
    context.idempotencyKey(),
  );
  context.out(index);
}

async function get(context: CommandContext, args: CommandArgs): Promise<void> {
  const collectionId = args.requirePositional(0, "collection-id");
  const name = args.requirePositional(1, "name");
  const version = integerPositional(args, 2, "version");
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  context.out(
    await client.getCollectionIndex(projectId, environmentId, collectionId, name, version),
  );
}

async function remove(context: CommandContext, args: CommandArgs): Promise<void> {
  const collectionId = args.requirePositional(0, "collection-id");
  const name = args.requirePositional(1, "name");
  const version = integerPositional(args, 2, "version");
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  context.out(
    await client.deleteCollectionIndex(projectId, environmentId, collectionId, name, version),
  );
}

export const indexesCommands: readonly Command[] = [
  {
    path: ["indexes", "list"],
    summary: "List a collection's indexes and their build state",
    operations: ["listCollectionIndexes"],
    positionals: [COLLECTION_ID],
    options: { ...TENANT_OPTIONS },
    run: list,
  },
  {
    path: ["indexes", "create"],
    summary: "Create an index; it is built in the background and reported active when ready",
    operations: ["createCollectionIndex"],
    positionals: [COLLECTION_ID],
    options: {
      ...TENANT_OPTIONS,
      name: { type: "string", required: true, description: "Index name", placeholder: "<name>" },
      version: {
        type: "string",
        required: true,
        description: "Index version (positive integer)",
        placeholder: "<n>",
      },
      field: {
        type: "string",
        multiple: true,
        description: "Indexed field as path[:ascending|descending]; repeat in key order",
        placeholder: "<path[:direction]>",
      },
      unique: { type: "boolean", description: "Reject documents with duplicate values" },
    },
    run: create,
  },
  {
    path: ["indexes", "get"],
    summary: "Show one index version, its fields, progress, and any failure",
    operations: ["getCollectionIndex"],
    positionals: [COLLECTION_ID, INDEX_NAME, INDEX_VERSION],
    options: { ...TENANT_OPTIONS },
    run: get,
  },
  {
    path: ["indexes", "delete"],
    summary: "Delete one index version",
    operations: ["deleteCollectionIndex"],
    positionals: [COLLECTION_ID, INDEX_NAME, INDEX_VERSION],
    options: { ...TENANT_OPTIONS },
    destructive: {
      action: "delete index",
      resource: (args) =>
        `${args.requirePositional(1, "name")}/${args.requirePositional(2, "version")}`,
    },
    run: remove,
  },
];
