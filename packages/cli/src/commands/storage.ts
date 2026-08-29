import type {
  CreateStorageBucketRequest,
  StorageBucketAccess,
  StorageBucketRule,
  UpdateStorageBucketRequest,
} from "@mako-cloud/management-sdk";

import type { CommandContext } from "../cli/context.js";
import { usageError } from "../cli/errors.js";
import type { TableColumn } from "../cli/output.js";
import type { Command, CommandArgs, OptionSpec, PositionalSpec } from "../cli/registry.js";
import { isJsonObject } from "./collections.js";
import { TENANT_OPTIONS, tenantFrom } from "./shared.js";

type RuleOperation = StorageBucketRule["operations"][number];

const ACCESS_MODES: readonly StorageBucketAccess[] = ["policy", "public"];
const RULE_OPERATIONS: readonly RuleOperation[] = ["create", "read", "update", "delete"];

/** The API's default for a new bucket's largest object: 1 MiB. */
const DEFAULT_MAX_OBJECT_BYTES = 1_048_576;
/** The platform ceiling on one object: 16 MiB. */
const MAX_OBJECT_BYTES = 16_777_216;
/** The API's ceiling on one page of objects. */
const MAX_LIST_LIMIT = 1000;

const BUCKET_COLUMNS: readonly TableColumn[] = [
  { key: "id" },
  { key: "access" },
  { key: "objectCount", label: "objects" },
  { key: "totalBytes", label: "bytes" },
  { key: "maxObjectBytes" },
  { key: "updatedAt" },
];

const OBJECT_COLUMNS: readonly TableColumn[] = [
  { key: "path" },
  { key: "contentType" },
  { key: "sizeBytes" },
  { key: "ownerId" },
  { key: "updatedAt" },
];

const BUCKET_ID: PositionalSpec = {
  name: "bucket-id",
  description: "Bucket id",
  required: true,
};

const OBJECT_PATH: PositionalSpec = {
  name: "path",
  description: "The object's /-separated path within the bucket",
  required: true,
};

/** Options `buckets create` and `buckets update` share: the bucket's configuration. */
const BUCKET_OPTIONS: Readonly<Record<string, OptionSpec>> = {
  access: {
    type: "string",
    description:
      "policy: rules govern every request; public: reads need no session, writes are still governed",
    placeholder: "<policy|public>",
  },
  "max-object-bytes": {
    type: "string",
    description: `Largest object the bucket accepts, in bytes (at most ${MAX_OBJECT_BYTES})`,
    placeholder: "<n>",
  },
  "content-type": {
    type: "string",
    multiple: true,
    description:
      'Allowed media type or type/* pattern; repeat for several, or pass "" alone to allow every type',
    placeholder: "<type>",
  },
  rules: {
    type: "string",
    description:
      "Policy rules JSON array [{id, effect, operations, expression}]: @path, - for stdin, or inline",
    placeholder: "<@file|-|json>",
  },
};

function bucketOf(args: CommandArgs): string {
  return args.requirePositional(0, "bucket-id");
}

/** The object path as typed; a path that could leave its bucket is refused here, unsent. */
function objectPathOf(args: CommandArgs): string {
  const path = args.requirePositional(1, "path");
  const segments = path.split("/");
  if (segments.some((segment) => segment === "" || segment === "." || segment === "..")) {
    throw usageError("<path> must not have empty, . or .. segments");
  }
  const control = Array.from(path).some((character) => {
    const code = character.codePointAt(0) ?? 0;
    return code < 0x20 || code === 0x7f;
  });
  if (control) throw usageError("<path> must not contain control characters");
  return path;
}

function accessFrom(args: CommandArgs): StorageBucketAccess | undefined {
  const access = args.string("access");
  if (access === undefined) return undefined;
  if (!ACCESS_MODES.includes(access as StorageBucketAccess)) {
    throw usageError(`--access must be one of ${ACCESS_MODES.join(", ")}`);
  }
  return access as StorageBucketAccess;
}

function maxObjectBytesFrom(args: CommandArgs): number | undefined {
  const bytes = args.integer("max-object-bytes");
  if (bytes === undefined) return undefined;
  if (bytes < 1 || bytes > MAX_OBJECT_BYTES) {
    throw usageError(`--max-object-bytes must be between 1 and ${MAX_OBJECT_BYTES}`);
  }
  return bytes;
}

/** The content types given, or `undefined` when the option was not used at all. */
function contentTypesFrom(args: CommandArgs): string[] | undefined {
  if (args.values["content-type"] === undefined) return undefined;
  return [...new Set(args.strings("content-type").map((type) => type.trim()))].filter(
    (type) => type !== "",
  );
}

function isRuleOperation(value: unknown): value is RuleOperation {
  return RULE_OPERATIONS.includes(value as RuleOperation);
}

function ruleFrom(value: unknown, index: number): StorageBucketRule {
  const at = `--rules[${index}]`;
  if (!isJsonObject(value)) {
    throw usageError(`${at} must be an object {id, effect, operations, expression}`);
  }
  const { id, effect, operations, expression } = value;
  if (typeof id !== "string" || id === "") throw usageError(`${at}.id must be a non-empty string`);
  if (effect !== "allow" && effect !== "deny") {
    throw usageError(`${at}.effect must be allow or deny`);
  }
  if (!Array.isArray(operations) || operations.length === 0 || !operations.every(isRuleOperation)) {
    throw usageError(`${at}.operations must list one or more of ${RULE_OPERATIONS.join(", ")}`);
  }
  if (typeof expression !== "string" || expression === "") {
    throw usageError(`${at}.expression must be a non-empty string`);
  }
  return { id, effect, operations, expression };
}

async function rulesFrom(
  context: CommandContext,
  args: CommandArgs,
): Promise<StorageBucketRule[] | undefined> {
  const source = args.string("rules");
  if (source === undefined) return undefined;
  const value = await context.readJson(source, "--rules");
  if (!Array.isArray(value)) throw usageError("--rules must be a JSON array of rules");
  return value.map(ruleFrom);
}

async function listBuckets(context: CommandContext, args: CommandArgs): Promise<void> {
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  const buckets = await client.listStorageBuckets(projectId, environmentId);
  context.out(buckets, { columns: BUCKET_COLUMNS });
}

async function createBucket(context: CommandContext, args: CommandArgs): Promise<void> {
  const id = bucketOf(args);
  const { projectId, environmentId } = tenantFrom(context, args);
  const access = accessFrom(args);
  const allowedContentTypes = contentTypesFrom(args);
  const rules = await rulesFrom(context, args);
  const request: CreateStorageBucketRequest = {
    id,
    maxObjectBytes: maxObjectBytesFrom(args) ?? DEFAULT_MAX_OBJECT_BYTES,
    ...(access !== undefined ? { access } : {}),
    ...(allowedContentTypes !== undefined ? { allowedContentTypes } : {}),
    ...(rules !== undefined ? { rules } : {}),
  };
  const client = await context.management();
  const bucket = await client.createStorageBucket(
    projectId,
    environmentId,
    request,
    context.idempotencyKey(),
  );
  context.out(bucket);
}

async function getBucket(context: CommandContext, args: CommandArgs): Promise<void> {
  const bucketId = bucketOf(args);
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  context.out(await client.getStorageBucket(projectId, environmentId, bucketId));
}

async function updateBucket(context: CommandContext, args: CommandArgs): Promise<void> {
  const bucketId = bucketOf(args);
  const { projectId, environmentId } = tenantFrom(context, args);
  const access = accessFrom(args);
  const maxObjectBytes = maxObjectBytesFrom(args);
  const allowedContentTypes = contentTypesFrom(args);
  const rules = await rulesFrom(context, args);
  // Only what was given is sent; the API leaves the rest as it is.
  const request: UpdateStorageBucketRequest = {
    ...(access !== undefined ? { access } : {}),
    ...(maxObjectBytes !== undefined ? { maxObjectBytes } : {}),
    ...(allowedContentTypes !== undefined ? { allowedContentTypes } : {}),
    ...(rules !== undefined ? { rules } : {}),
  };
  if (Object.keys(request).length === 0) {
    throw usageError(
      "nothing to update: pass at least one of --access, --max-object-bytes, --content-type, --rules",
    );
  }
  const client = await context.management();
  const bucket = await client.updateStorageBucket(
    projectId,
    environmentId,
    bucketId,
    request,
    context.idempotencyKey(),
  );
  context.out(bucket);
}

async function deleteBucket(context: CommandContext, args: CommandArgs): Promise<void> {
  const bucketId = bucketOf(args);
  const { projectId, environmentId } = tenantFrom(context, args);
  const deleteObjects = args.boolean("delete-objects");
  const client = await context.management();
  const removal = await client.deleteStorageBucket(
    projectId,
    environmentId,
    bucketId,
    `delete:${bucketId}`,
    deleteObjects,
  );
  context.info(
    `bucket ${bucketId} deleted: ${removal.objectCount} objects, ${removal.totalBytes} bytes removed`,
  );
  context.out(removal);
}

async function listObjects(context: CommandContext, args: CommandArgs): Promise<void> {
  const bucketId = bucketOf(args);
  const { projectId, environmentId } = tenantFrom(context, args);
  const prefix = args.string("prefix");
  const limit = args.integer("limit");
  if (limit !== undefined && (limit < 1 || limit > MAX_LIST_LIMIT)) {
    throw usageError(`--limit must be between 1 and ${MAX_LIST_LIMIT}`);
  }
  const start = args.string("cursor");
  const client = await context.management();
  const page = await context.collect((cursor) => {
    const next = cursor ?? start;
    return client.listStorageObjects(projectId, environmentId, bucketId, {
      ...(prefix !== undefined ? { prefix } : {}),
      ...(limit !== undefined ? { limit } : {}),
      ...(next !== undefined ? { cursor: next } : {}),
    });
  });
  context.out(page, { columns: OBJECT_COLUMNS });
}

async function deleteObject(context: CommandContext, args: CommandArgs): Promise<void> {
  const bucketId = bucketOf(args);
  const path = objectPathOf(args);
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  context.out(await client.deleteStorageObject(projectId, environmentId, bucketId, path));
}

export const storageCommands: readonly Command[] = [
  {
    path: ["storage", "buckets", "list"],
    summary: "List the environment's storage buckets with their object counts and totals",
    operations: ["listStorageBuckets"],
    options: { ...TENANT_OPTIONS },
    run: listBuckets,
  },
  {
    path: ["storage", "buckets", "create"],
    summary: "Create a storage bucket; without rules a policy bucket refuses every request",
    operations: ["createStorageBucket"],
    positionals: [BUCKET_ID],
    options: { ...TENANT_OPTIONS, ...BUCKET_OPTIONS },
    run: createBucket,
  },
  {
    path: ["storage", "buckets", "get"],
    summary: "Show a storage bucket, its limits, its rules, and its totals",
    operations: ["getStorageBucket"],
    positionals: [BUCKET_ID],
    options: { ...TENANT_OPTIONS },
    run: getBucket,
  },
  {
    path: ["storage", "buckets", "update"],
    summary:
      "Change a storage bucket's access, limits, content types, or rules; only given options are sent",
    operations: ["updateStorageBucket"],
    positionals: [BUCKET_ID],
    options: { ...TENANT_OPTIONS, ...BUCKET_OPTIONS },
    run: updateBucket,
  },
  {
    path: ["storage", "buckets", "delete"],
    summary:
      "Delete a storage bucket; one that still holds objects is refused unless --delete-objects confirms their loss",
    operations: ["deleteStorageBucket"],
    positionals: [BUCKET_ID],
    options: {
      ...TENANT_OPTIONS,
      "delete-objects": {
        type: "boolean",
        description: "Delete every object the bucket holds along with it",
      },
    },
    destructive: { action: "delete bucket", resource: bucketOf },
    run: deleteBucket,
  },
  {
    path: ["storage", "objects", "list"],
    summary: "List a bucket's objects in path order; --all follows the cursor to the end",
    operations: ["listStorageObjects"],
    positionals: [BUCKET_ID],
    options: {
      ...TENANT_OPTIONS,
      prefix: {
        type: "string",
        description: "Only objects whose path starts with this prefix",
        placeholder: "<prefix>",
      },
      limit: {
        type: "string",
        description: `Objects per page, 1-${MAX_LIST_LIMIT} (default: 100)`,
        placeholder: "<n>",
      },
      cursor: {
        type: "string",
        description: "Continue from a previous page's next cursor",
        placeholder: "<cursor>",
      },
    },
    run: listObjects,
  },
  {
    path: ["storage", "objects", "delete"],
    summary: "Delete one object from a bucket by its path",
    operations: ["deleteStorageObject"],
    positionals: [BUCKET_ID, OBJECT_PATH],
    options: { ...TENANT_OPTIONS },
    destructive: { action: "delete object", resource: objectPathOf },
    run: deleteObject,
  },
];
