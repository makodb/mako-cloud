import { randomUUID } from "node:crypto";

import type { ProjectCredentialIssue, ServiceCredentialScope } from "@mako-cloud/management-sdk";

import type { CommandContext } from "../cli/context.js";
import { usageError } from "../cli/errors.js";
import type { TableColumn } from "../cli/output.js";
import type { Command, CommandArgs, OptionSpec, PositionalSpec } from "../cli/registry.js";
import { SECRET_FILE_OPTION, TENANT_OPTIONS, tenantFrom } from "./shared.js";

type DocumentOperation = ServiceCredentialScope["operations"][number];

const DOCUMENT_OPERATIONS: readonly DocumentOperation[] = ["create", "read", "update", "delete"];

/** The API's ceiling on an overlap window: thirty days. */
const MAX_OVERLAP_SECONDS = 2_592_000;

const SIGNING_KEY_COLUMNS: readonly TableColumn[] = [
  { key: "keyId" },
  { key: "state" },
  { key: "createdAt" },
  { key: "retireAt" },
];

const CREDENTIAL_ID: PositionalSpec = {
  name: "credential-id",
  description: "Credential id",
  required: true,
};

const OVERLAP_OPTION: OptionSpec = {
  type: "string",
  required: true,
  description: `Seconds the old key keeps working alongside the new one (at most ${MAX_OVERLAP_SECONDS})`,
  placeholder: "<seconds>",
};

function credentialOf(args: CommandArgs): string {
  return args.requirePositional(0, "credential-id");
}

/** The console's shape for a generated id: a prefix and twelve hex characters. */
function generatedId(prefix: string): string {
  return `${prefix}_${randomUUID().replace(/-/gu, "").slice(0, 12)}`;
}

function idFrom(args: CommandArgs, prefix: string): string {
  const id = args.string("id");
  if (id === undefined) return generatedId(prefix);
  if (!/^[A-Za-z0-9_-]{1,128}$/u.test(id)) {
    throw usageError("--id may only contain letters, digits, _ and - (1-128 characters)");
  }
  return id;
}

function splitList(value: string): string[] {
  return value
    .split(",")
    .map((item) => item.trim())
    .filter((item) => item !== "");
}

function scopeFrom(args: CommandArgs): ServiceCredentialScope {
  const collections = [...new Set(args.strings("collection").flatMap(splitList))];
  if (collections.length === 0) {
    throw usageError("at least one --collection <collection-id> is required");
  }
  const operations = [...new Set(args.strings("operation").flatMap(splitList))];
  if (operations.length === 0) {
    throw usageError(`at least one --operation is required (${DOCUMENT_OPERATIONS.join(", ")})`);
  }
  const unknown = operations.find(
    (operation) => !DOCUMENT_OPERATIONS.includes(operation as DocumentOperation),
  );
  if (unknown !== undefined) {
    throw usageError(
      `--operation must be one of ${DOCUMENT_OPERATIONS.join(", ")}, got "${unknown}"`,
    );
  }
  return { collections, operations: operations as DocumentOperation[] };
}

function overlapFrom(args: CommandArgs, minimum: number): number {
  const overlap = args.integer("overlap");
  if (overlap === undefined) throw usageError("--overlap <seconds> is required");
  if (overlap < minimum || overlap > MAX_OVERLAP_SECONDS) {
    throw usageError(`--overlap must be between ${minimum} and ${MAX_OVERLAP_SECONDS} seconds`);
  }
  return overlap;
}

/** Prints an issued credential: the record once, the secret exactly once. */
async function issued(
  context: CommandContext,
  args: CommandArgs,
  label: string,
  issue: ProjectCredentialIssue,
): Promise<void> {
  await context.secret(
    `${label} ${issue.credential.id}`,
    issue.value,
    { ...issue.credential },
    args.string("secret-file"),
  );
}

async function createPublic(context: CommandContext, args: CommandArgs): Promise<void> {
  const { projectId, environmentId } = tenantFrom(context, args);
  const id = idFrom(args, "pk");
  const client = await context.management();
  const issue = await client.createPublicProjectKey(
    projectId,
    environmentId,
    id,
    context.idempotencyKey(),
  );
  await issued(context, args, "public key", issue);
}

async function createService(context: CommandContext, args: CommandArgs): Promise<void> {
  const { projectId, environmentId } = tenantFrom(context, args);
  const id = idFrom(args, "sk");
  const scope = scopeFrom(args);
  const client = await context.management();
  const issue = await client.createServiceCredential(
    projectId,
    environmentId,
    id,
    scope,
    context.idempotencyKey(),
  );
  await issued(context, args, "service credential", issue);
}

async function get(context: CommandContext, args: CommandArgs): Promise<void> {
  const credentialId = credentialOf(args);
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  context.out(await client.getProjectCredential(projectId, environmentId, credentialId));
}

async function retire(context: CommandContext, args: CommandArgs): Promise<void> {
  const credentialId = credentialOf(args);
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  await client.retireProjectCredential(projectId, environmentId, credentialId);
  if (context.json) context.out({ id: credentialId, state: "retired" });
  else context.info(`credential ${credentialId} retired`);
}

async function rotate(context: CommandContext, args: CommandArgs): Promise<void> {
  const credentialId = credentialOf(args);
  const { projectId, environmentId } = tenantFrom(context, args);
  const replacementId = args.requireString("replacement-id");
  const overlapSeconds = overlapFrom(args, 0);
  const client = await context.management();
  const issue = await client.rotateProjectCredential(
    projectId,
    environmentId,
    credentialId,
    replacementId,
    overlapSeconds,
    context.idempotencyKey(),
  );
  await issued(context, args, "replacement credential", issue);
}

async function signingInit(context: CommandContext, args: CommandArgs): Promise<void> {
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  context.out(
    await client.initializeJwtSigningKey(projectId, environmentId, context.idempotencyKey()),
  );
}

async function signingList(context: CommandContext, args: CommandArgs): Promise<void> {
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  context.out(await client.listJwtSigningKeys(projectId, environmentId), {
    columns: SIGNING_KEY_COLUMNS,
  });
}

async function signingRotate(context: CommandContext, args: CommandArgs): Promise<void> {
  const { projectId, environmentId } = tenantFrom(context, args);
  const overlapSeconds = overlapFrom(args, 1);
  const client = await context.management();
  context.out(
    await client.rotateJwtSigningKey(
      projectId,
      environmentId,
      overlapSeconds,
      context.idempotencyKey(),
    ),
  );
}

export const keysCommands: readonly Command[] = [
  {
    path: ["keys", "public", "create"],
    summary: "Issue a public key for client apps; the secret is shown once",
    operations: ["createPublicProjectKey"],
    options: {
      ...TENANT_OPTIONS,
      id: {
        type: "string",
        description: "Credential id (default: a generated pk_... id)",
        placeholder: "<credential-id>",
      },
      ...SECRET_FILE_OPTION,
    },
    run: createPublic,
  },
  {
    path: ["keys", "service", "create"],
    summary: "Issue a service credential scoped to collections and operations; shown once",
    operations: ["createServiceCredential"],
    options: {
      ...TENANT_OPTIONS,
      id: {
        type: "string",
        description: "Credential id (default: a generated sk_... id)",
        placeholder: "<credential-id>",
      },
      collection: {
        type: "string",
        multiple: true,
        description: "Collection the credential may reach; repeat or comma-separate",
        placeholder: "<collection-id>",
      },
      operation: {
        type: "string",
        multiple: true,
        description: `Allowed operation (${DOCUMENT_OPERATIONS.join(", ")}); repeat or comma-separate`,
        placeholder: "<operation>",
      },
      ...SECRET_FILE_OPTION,
    },
    run: createService,
  },
  {
    path: ["keys", "get"],
    summary: "Show a credential's kind, scope, and state (never its secret)",
    operations: ["getProjectCredential"],
    positionals: [CREDENTIAL_ID],
    options: { ...TENANT_OPTIONS },
    run: get,
  },
  {
    path: ["keys", "retire"],
    summary: "Retire a credential; requests signed with it are refused from then on",
    operations: ["retireProjectCredential"],
    positionals: [CREDENTIAL_ID],
    options: { ...TENANT_OPTIONS },
    destructive: { action: "retire credential", resource: credentialOf },
    run: retire,
  },
  {
    path: ["keys", "rotate"],
    summary: "Issue a replacement credential and retire this one after the overlap",
    operations: ["rotateProjectCredential"],
    positionals: [CREDENTIAL_ID],
    options: {
      ...TENANT_OPTIONS,
      "replacement-id": {
        type: "string",
        required: true,
        description: "Id of the replacement credential",
        placeholder: "<credential-id>",
      },
      overlap: OVERLAP_OPTION,
      ...SECRET_FILE_OPTION,
    },
    destructive: { action: "rotate credential", resource: credentialOf },
    run: rotate,
  },
  {
    path: ["keys", "signing", "init"],
    summary: "Create the environment's first JWT signing key",
    operations: ["initializeJwtSigningKey"],
    options: { ...TENANT_OPTIONS },
    run: signingInit,
  },
  {
    path: ["keys", "signing", "list"],
    summary: "List the environment's JWT signing keys and their states",
    operations: ["listJwtSigningKeys"],
    options: { ...TENANT_OPTIONS },
    run: signingList,
  },
  {
    path: ["keys", "signing", "rotate"],
    summary: "Rotate the JWT signing key; the old key verifies tokens until the overlap ends",
    operations: ["rotateJwtSigningKey"],
    options: { ...TENANT_OPTIONS, overlap: OVERLAP_OPTION },
    destructive: { action: "rotate the JWT signing key", resource: () => "signing-key" },
    run: signingRotate,
  },
];
