import type { CreatePolicyDraftRequest, PolicyExample } from "@mako-cloud/management-sdk";

import type { CommandContext } from "../cli/context.js";
import { usageError } from "../cli/errors.js";
import type { TableColumn } from "../cli/output.js";
import type { Command, CommandArgs, PositionalSpec } from "../cli/registry.js";
import { isJsonObject } from "./collections.js";
import { integerPositional } from "./indexes.js";
import { TENANT_OPTIONS, tenantFrom } from "./shared.js";

const RESULT_COLUMNS: readonly TableColumn[] = [
  { key: "allowed" },
  { key: "code" },
  { key: "evaluatedRules" },
  { key: "matchedRuleIds" },
];

const COLLECTION_ID: PositionalSpec = {
  name: "collection-id",
  description: "Collection id",
  required: true,
};

const POLICY_VERSION: PositionalSpec = {
  name: "version",
  description: "Policy version",
  required: true,
};

function collectionOf(args: CommandArgs): string {
  return args.requirePositional(0, "collection-id");
}

async function get(context: CommandContext, args: CommandArgs): Promise<void> {
  const collectionId = collectionOf(args);
  const { projectId, environmentId } = tenantFrom(context, args);
  const version = args.integer("version");
  const client = await context.management();
  if (version === undefined) {
    context.out(await client.getActiveCollectionPolicy(projectId, environmentId, collectionId));
    return;
  }
  context.out(await client.getCollectionPolicy(projectId, environmentId, collectionId, version));
}

async function draft(context: CommandContext, args: CommandArgs): Promise<void> {
  const collectionId = collectionOf(args);
  const { projectId, environmentId } = tenantFrom(context, args);
  const input = await context.readJson(args.requireString("input"), "--input");
  if (!isJsonObject(input)) throw usageError("--input must be a JSON object {version, rules}");
  const { version, rules } = input;
  if (!Number.isInteger(version) || (version as number) < 1) {
    throw usageError("--input must carry a positive integer version");
  }
  if (!Array.isArray(rules) || rules.length === 0) {
    throw usageError("--input must carry a non-empty rules array");
  }
  const request: CreatePolicyDraftRequest = {
    version: version as number,
    rules: rules as CreatePolicyDraftRequest["rules"],
  };
  const client = await context.management();
  const policy = await client.createCollectionPolicyDraft(
    projectId,
    environmentId,
    collectionId,
    request,
    context.idempotencyKey(),
  );
  context.out(policy);
}

async function validate(context: CommandContext, args: CommandArgs): Promise<void> {
  const collectionId = collectionOf(args);
  const version = integerPositional(args, 1, "version");
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  context.out(
    await client.validateCollectionPolicy(projectId, environmentId, collectionId, version),
  );
}

async function testPolicy(context: CommandContext, args: CommandArgs): Promise<void> {
  const collectionId = collectionOf(args);
  const version = integerPositional(args, 1, "version");
  const { projectId, environmentId } = tenantFrom(context, args);
  const input = await context.readJson(args.requireString("examples"), "--examples");
  const examples = isJsonObject(input) && Array.isArray(input.examples) ? input.examples : input;
  if (!Array.isArray(examples) || examples.length === 0) {
    throw usageError("--examples must be a non-empty JSON array of examples, or {examples: [...]}");
  }
  const client = await context.management();
  const results = await client.testCollectionPolicy(
    projectId,
    environmentId,
    collectionId,
    version,
    examples as PolicyExample[],
  );
  context.out(results, { columns: RESULT_COLUMNS });
}

async function activate(context: CommandContext, args: CommandArgs): Promise<void> {
  const collectionId = collectionOf(args);
  const version = integerPositional(args, 1, "version");
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  context.out(
    await client.activateCollectionPolicy(
      projectId,
      environmentId,
      collectionId,
      version,
      context.idempotencyKey(),
    ),
  );
}

async function rollback(context: CommandContext, args: CommandArgs): Promise<void> {
  const collectionId = collectionOf(args);
  const version = integerPositional(args, 1, "version");
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  context.out(
    await client.rollbackCollectionPolicy(
      projectId,
      environmentId,
      collectionId,
      version,
      context.idempotencyKey(),
    ),
  );
}

export const policiesCommands: readonly Command[] = [
  {
    path: ["policies", "get"],
    summary: "Show the active policy of a collection, or one version with --version",
    operations: ["getActiveCollectionPolicy", "getCollectionPolicy"],
    positionals: [COLLECTION_ID],
    options: {
      ...TENANT_OPTIONS,
      version: {
        type: "string",
        description: "Show this policy version instead of the active one",
        placeholder: "<n>",
      },
    },
    run: get,
  },
  {
    path: ["policies", "draft"],
    summary: "Create a policy draft from {version, rules}",
    operations: ["createCollectionPolicyDraft"],
    positionals: [COLLECTION_ID],
    options: {
      ...TENANT_OPTIONS,
      input: {
        type: "string",
        required: true,
        description:
          "Draft JSON {version, rules: [{id, effect, operations, expression}]}: @path, - for stdin, or inline",
        placeholder: "<@file|-|json>",
      },
    },
    run: draft,
  },
  {
    path: ["policies", "validate"],
    summary: "Validate a policy version and report its diagnostics",
    operations: ["validateCollectionPolicy"],
    positionals: [COLLECTION_ID, POLICY_VERSION],
    options: { ...TENANT_OPTIONS },
    run: validate,
  },
  {
    path: ["policies", "test"],
    summary: "Evaluate a policy version against example requests",
    operations: ["testCollectionPolicy"],
    positionals: [COLLECTION_ID, POLICY_VERSION],
    options: {
      ...TENANT_OPTIONS,
      examples: {
        type: "string",
        required: true,
        description:
          "Examples JSON [{operation, identity: {role, trustedClaims, userId?}, oldDocument?, newDocument?}]: @path, - for stdin, or inline",
        placeholder: "<@file|-|json>",
      },
    },
    run: testPolicy,
  },
  {
    path: ["policies", "activate"],
    summary: "Activate a policy version; every client is authorized by it from then on",
    operations: ["activateCollectionPolicy"],
    positionals: [COLLECTION_ID, POLICY_VERSION],
    options: { ...TENANT_OPTIONS },
    destructive: { action: "activate the policy of collection", resource: collectionOf },
    run: activate,
  },
  {
    path: ["policies", "rollback"],
    summary: "Make an earlier policy version active again",
    operations: ["rollbackCollectionPolicy"],
    positionals: [COLLECTION_ID, POLICY_VERSION],
    options: { ...TENANT_OPTIONS },
    destructive: { action: "revert the policy of collection", resource: collectionOf },
    run: rollback,
  },
];
