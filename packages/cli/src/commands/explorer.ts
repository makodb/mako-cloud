import type {
  ExplorerGrantRequest,
  ExplorerMutationRequest,
  ExplorerQueryRequest,
  MakoManagementClient,
} from "@mako-cloud/management-sdk";

import type { CommandContext } from "../cli/context.js";
import { CliError, EXIT, usageError } from "../cli/errors.js";
import type { TableColumn } from "../cli/output.js";
import type { Command, CommandArgs, OptionSpec } from "../cli/registry.js";
import { TENANT_OPTIONS, type Tenant, tenantFrom } from "./shared.js";

type ExplorerOperation = ExplorerGrantRequest["operations"][number];
type ExplorerAccessMode = ExplorerGrantRequest["mode"];

/** A grant only has to outlive one command; the API caps it at 300 seconds. */
const DEFAULT_DURATION_SECONDS = 60;
const MAX_DURATION_SECONDS = 300;
const DEFAULT_PAGE_LIMIT = 25;

const GRANT_OPTIONS: Readonly<Record<string, OptionSpec>> = {
  ...TENANT_OPTIONS,
  mode: {
    type: "string",
    description:
      "policy_preview or administrative (default: policy_preview with --as-user, otherwise administrative)",
    placeholder: "<mode>",
  },
  "as-user": {
    type: "string",
    description: "Evaluate document policies as this application user (policy preview)",
    placeholder: "<application-user-id>",
  },
  reason: {
    type: "string",
    description: "Reason recorded in the audit trail (required for administrative access)",
    placeholder: "<text>",
  },
  duration: {
    type: "string",
    description: `Grant lifetime in seconds, 1-${MAX_DURATION_SECONDS} (default: ${DEFAULT_DURATION_SECONDS}); the grant is revoked when the command ends`,
    placeholder: "<seconds>",
  },
};

const PAGE_OPTIONS: Readonly<Record<string, OptionSpec>> = {
  limit: {
    type: "string",
    description: `Documents per page, 1-200 (default: ${DEFAULT_PAGE_LIMIT})`,
    placeholder: "<n>",
  },
  cursor: {
    type: "string",
    description: "Continue from a page cursor",
    placeholder: "<cursor>",
  },
};

const SCHEMA_VERSION_OPTION: Readonly<Record<string, OptionSpec>> = {
  "schema-version": {
    type: "string",
    description: "Active schema version the mutation targets (overrides the JSON's schemaVersion)",
    placeholder: "<n>",
  },
};

const COLLECTION_POSITIONAL = {
  name: "collection-id",
  description: "Collection id",
  required: true,
};
const DOCUMENT_POSITIONAL = { name: "document-id", description: "Document id", required: true };

const DOCUMENT_COLUMNS: readonly TableColumn[] = [
  { key: "documentId" },
  { key: "revision" },
  { key: "schemaVersion" },
  { key: "deleted" },
  { key: "content" },
];

const REVISION_COLUMNS: readonly TableColumn[] = [
  { key: "revision" },
  { key: "schemaVersion" },
  { key: "commitPosition" },
  { key: "committedAtUnixSeconds", label: "committedAt" },
  { key: "deleted" },
  { key: "retainedUntilUnixSeconds", label: "retainedUntil" },
];

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function modeFrom(explicit: string | undefined, asUser: string | undefined): ExplorerAccessMode {
  if (explicit === undefined) return asUser !== undefined ? "policy_preview" : "administrative";
  if (explicit === "policy_preview" || explicit === "administrative") return explicit;
  throw usageError("--mode must be policy_preview or administrative");
}

/**
 * The grant request for one command: policy preview evaluates as an application
 * user and records no reason; administrative access bypasses document policies
 * and must carry a reason for the audit trail. The API enforces the same rules;
 * checking here fails before anything is sent.
 */
function grantRequestFrom(
  tenant: Tenant,
  collectionId: string,
  args: CommandArgs,
  operations: readonly ExplorerOperation[],
): ExplorerGrantRequest {
  const asUser = args.string("as-user");
  const reason = args.string("reason");
  const mode = modeFrom(args.string("mode"), asUser);
  if (mode === "policy_preview") {
    if (asUser === undefined) {
      throw usageError("policy preview needs --as-user <application-user-id>");
    }
    if (reason !== undefined) {
      throw usageError(
        "policy preview takes no --reason; a reason is recorded for administrative access",
      );
    }
  } else {
    if (reason === undefined || reason.trim().length < 3) {
      throw usageError(
        "administrative access needs --reason <text> (at least 3 characters); it is recorded in the audit trail",
      );
    }
    if (asUser !== undefined) {
      throw usageError("administrative access bypasses document policies and takes no --as-user");
    }
  }
  const durationSeconds = args.integer("duration") ?? DEFAULT_DURATION_SECONDS;
  if (durationSeconds < 1 || durationSeconds > MAX_DURATION_SECONDS) {
    throw usageError(`--duration must be between 1 and ${MAX_DURATION_SECONDS} seconds`);
  }
  return {
    tenant: { projectId: tenant.projectId, environmentId: tenant.environmentId },
    collectionId,
    mode,
    operations: [...operations],
    applicationUserId: mode === "policy_preview" ? (asUser ?? null) : null,
    reason: mode === "administrative" ? (reason ?? null) : null,
    durationSeconds,
  };
}

type GrantedAction<T> = (
  client: MakoManagementClient,
  tenant: Tenant,
  collectionId: string,
  capability: string,
) => Promise<T>;

/**
 * Issues a grant scoped to the collection and operations one command needs,
 * runs the command with the capability, and revokes the grant whether or not
 * the command succeeded. The capability exists only in this frame: it is never
 * printed, never placed in a URL, and never written anywhere.
 */
async function withGrant<T>(
  context: CommandContext,
  args: CommandArgs,
  operations: readonly ExplorerOperation[],
  action: GrantedAction<T>,
): Promise<T> {
  const tenant = tenantFrom(context, args);
  const collectionId = args.requirePositional(0, "collection-id");
  const request = grantRequestFrom(tenant, collectionId, args, operations);
  const client = await context.management();
  const grant = await client.issueExplorerGrant(tenant.projectId, tenant.environmentId, request);
  try {
    return await action(client, tenant, collectionId, grant.capability);
  } finally {
    try {
      await client.revokeExplorerGrant(tenant.projectId, tenant.environmentId, grant.grantId);
    } catch {
      // Expiry and server-side revocation remain authoritative; the command's
      // own outcome is what the caller needs to see.
      context.info(
        `explorer grant ${grant.grantId} could not be revoked; it expires at ${new Date(
          grant.expiresAtUnixSeconds * 1000,
        ).toISOString()}`,
      );
    }
  }
}

function pageLimit(args: CommandArgs, fallback = DEFAULT_PAGE_LIMIT): number {
  const limit = args.integer("limit") ?? fallback;
  if (limit < 1 || limit > 200) throw usageError("--limit must be between 1 and 200");
  return limit;
}

async function queryFrom(
  context: CommandContext,
  args: CommandArgs,
): Promise<ExplorerQueryRequest> {
  const input = await context.readJson<unknown>(args.requireString("query"), "--query");
  if (!isRecord(input)) throw usageError("--query must be a JSON object with predicates and sort");
  const predicates = input.predicates ?? [];
  const sort = input.sort ?? [];
  if (!Array.isArray(predicates) || !Array.isArray(sort)) {
    throw usageError("--query predicates and sort must be arrays");
  }
  const cursor = args.string("cursor") ?? (typeof input.cursor === "string" ? input.cursor : null);
  const inlineLimit = typeof input.limit === "number" ? input.limit : DEFAULT_PAGE_LIMIT;
  return {
    predicates: predicates as ExplorerQueryRequest["predicates"],
    sort: sort as ExplorerQueryRequest["sort"],
    limit: pageLimit(args, inlineLimit),
    cursor,
  };
}

async function mutationFrom(
  context: CommandContext,
  args: CommandArgs,
): Promise<ExplorerMutationRequest> {
  const input = await context.readJson<unknown>(args.requireString("mutation"), "--mutation");
  if (!isRecord(input)) throw usageError("--mutation must be a JSON object");
  const { kind, documentId } = input;
  if (kind !== "create" && kind !== "update" && kind !== "delete") {
    throw usageError('--mutation needs "kind": create, update, or delete');
  }
  if (typeof documentId !== "string" || documentId === "") {
    throw usageError('--mutation needs a "documentId"');
  }
  const schemaVersion = args.integer("schema-version") ?? input.schemaVersion;
  if (typeof schemaVersion !== "number" || !Number.isInteger(schemaVersion) || schemaVersion < 1) {
    throw usageError('--mutation needs a positive "schemaVersion" (or pass --schema-version)');
  }
  const idempotencyKey =
    typeof input.idempotencyKey === "string" && input.idempotencyKey.length >= 16
      ? input.idempotencyKey
      : context.idempotencyKey();
  return {
    kind,
    documentId,
    schemaVersion,
    expectedRevision: typeof input.expectedRevision === "string" ? input.expectedRevision : null,
    idempotencyKey,
    content: isRecord(input.content) ? input.content : null,
  };
}

async function getDocument(context: CommandContext, args: CommandArgs): Promise<void> {
  const documentId = args.requirePositional(1, "document-id");
  const document = await withGrant(
    context,
    args,
    ["get"],
    (client, tenant, collectionId, capability) =>
      client.explorerGetDocument(
        tenant.projectId,
        tenant.environmentId,
        collectionId,
        documentId,
        capability,
      ),
  );
  context.out(document);
}

async function browse(context: CommandContext, args: CommandArgs): Promise<void> {
  const limit = pageLimit(args);
  const includeRetainedTombstones = args.boolean("include-tombstones");
  const first = args.string("cursor") ?? null;
  const page = await withGrant(
    context,
    args,
    ["browse"],
    (client, tenant, collectionId, capability) =>
      context.collect((cursor) =>
        client.explorerBrowseDocuments(
          tenant.projectId,
          tenant.environmentId,
          collectionId,
          { limit, cursor: cursor ?? first, includeRetainedTombstones },
          capability,
        ),
      ),
  );
  context.out(page, { columns: DOCUMENT_COLUMNS });
}

async function plan(context: CommandContext, args: CommandArgs): Promise<void> {
  const request = await queryFrom(context, args);
  const result = await withGrant(
    context,
    args,
    ["plan"],
    (client, tenant, collectionId, capability) =>
      client.explorerPlanQuery(
        tenant.projectId,
        tenant.environmentId,
        collectionId,
        request,
        capability,
      ),
  );
  context.out(result);
}

async function query(context: CommandContext, args: CommandArgs): Promise<void> {
  const request = await queryFrom(context, args);
  const page = await withGrant(
    context,
    args,
    ["query"],
    (client, tenant, collectionId, capability) =>
      context.collect((cursor) =>
        client.explorerQueryDocuments(
          tenant.projectId,
          tenant.environmentId,
          collectionId,
          { ...request, cursor: cursor ?? request.cursor },
          capability,
        ),
      ),
  );
  context.out(page, { columns: DOCUMENT_COLUMNS });
}

async function history(context: CommandContext, args: CommandArgs): Promise<void> {
  const documentId = args.requirePositional(1, "document-id");
  const revisions = await withGrant(
    context,
    args,
    ["history"],
    (client, tenant, collectionId, capability) =>
      client.explorerDocumentHistory(
        tenant.projectId,
        tenant.environmentId,
        collectionId,
        documentId,
        capability,
      ),
  );
  context.out(revisions, { columns: REVISION_COLUMNS });
}

async function simulate(context: CommandContext, args: CommandArgs): Promise<void> {
  const mutation = await mutationFrom(context, args);
  const result = await withGrant(
    context,
    args,
    ["simulate"],
    (client, tenant, collectionId, capability) =>
      client.explorerSimulateMutation(
        tenant.projectId,
        tenant.environmentId,
        collectionId,
        mutation,
        capability,
      ),
  );
  context.out(result);
}

async function mutate(context: CommandContext, args: CommandArgs): Promise<void> {
  const mutation = await mutationFrom(context, args);
  const result = await withGrant(
    context,
    args,
    ["mutate"],
    (client, tenant, collectionId, capability) =>
      client.explorerMutateDocument(
        tenant.projectId,
        tenant.environmentId,
        collectionId,
        mutation,
        capability,
      ),
  );
  context.out(result);
  if (!result.committed) {
    // The result (with the current revision on a conflict) is the output a
    // script needs to reload, compare, and retry; the exit code says it did not land.
    throw new CliError(
      result.conflict !== null
        ? `the mutation was not committed: the current revision is ${result.conflict.revision}`
        : "the mutation was not committed",
      EXIT.refused,
      "CLI_MUTATION_NOT_COMMITTED",
    );
  }
}

const GRANT_OPERATIONS = ["issueExplorerGrant", "revokeExplorerGrant"] as const;

export const explorerCommands: readonly Command[] = [
  {
    path: ["explorer", "get"],
    summary: "Read one document under a short-lived explorer grant",
    operations: [...GRANT_OPERATIONS, "explorerGetDocument"],
    positionals: [COLLECTION_POSITIONAL, DOCUMENT_POSITIONAL],
    options: GRANT_OPTIONS,
    run: getDocument,
  },
  {
    path: ["explorer", "browse"],
    summary: "Page through a collection in primary-key order over a stable snapshot",
    operations: [...GRANT_OPERATIONS, "explorerBrowseDocuments"],
    positionals: [COLLECTION_POSITIONAL],
    options: {
      ...GRANT_OPTIONS,
      ...PAGE_OPTIONS,
      "include-tombstones": {
        type: "boolean",
        description: "Include retained tombstones (needs history permission)",
      },
    },
    run: browse,
  },
  {
    path: ["explorer", "plan"],
    summary: "Show which index a query would use, or the index it needs",
    operations: [...GRANT_OPERATIONS, "explorerPlanQuery"],
    positionals: [COLLECTION_POSITIONAL],
    options: {
      ...GRANT_OPTIONS,
      ...PAGE_OPTIONS,
      query: {
        type: "string",
        description: "Query JSON ({predicates, sort, limit, cursor}) as -, @path, or inline",
        placeholder: "<@file|-|json>",
        required: true,
      },
    },
    run: plan,
  },
  {
    path: ["explorer", "query"],
    summary: "Run an indexed query and page through its results",
    operations: [...GRANT_OPERATIONS, "explorerQueryDocuments"],
    positionals: [COLLECTION_POSITIONAL],
    options: {
      ...GRANT_OPTIONS,
      ...PAGE_OPTIONS,
      query: {
        type: "string",
        description: "Query JSON ({predicates, sort, limit, cursor}) as -, @path, or inline",
        placeholder: "<@file|-|json>",
        required: true,
      },
    },
    run: query,
  },
  {
    path: ["explorer", "history"],
    summary: "List the retained revisions and tombstones of one document",
    operations: [...GRANT_OPERATIONS, "explorerDocumentHistory"],
    positionals: [COLLECTION_POSITIONAL, DOCUMENT_POSITIONAL],
    options: GRANT_OPTIONS,
    run: history,
  },
  {
    path: ["explorer", "simulate"],
    summary: "Validate a mutation against the schema, revision, and policy without committing",
    operations: [...GRANT_OPERATIONS, "explorerSimulateMutation"],
    positionals: [COLLECTION_POSITIONAL],
    options: {
      ...GRANT_OPTIONS,
      ...SCHEMA_VERSION_OPTION,
      mutation: {
        type: "string",
        description:
          "Mutation JSON ({kind, documentId, expectedRevision, schemaVersion, content}) as -, @path, or inline",
        placeholder: "<@file|-|json>",
        required: true,
      },
    },
    run: simulate,
  },
  {
    path: ["explorer", "mutate"],
    summary: "Commit a create, update, or delete with administrative access",
    operations: [...GRANT_OPERATIONS, "explorerMutateDocument"],
    positionals: [COLLECTION_POSITIONAL],
    options: {
      ...GRANT_OPTIONS,
      ...SCHEMA_VERSION_OPTION,
      mutation: {
        type: "string",
        description:
          "Mutation JSON ({kind, documentId, expectedRevision, schemaVersion, content}) as -, @path, or inline",
        placeholder: "<@file|-|json>",
        required: true,
      },
    },
    destructive: {
      action: "mutate collection",
      resource: (args) => args.requirePositional(0, "collection-id"),
    },
    run: mutate,
  },
];
