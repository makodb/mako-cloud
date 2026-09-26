import type {
  Collection,
  CollectionIndex,
  MakoManagementClient,
  StorageBucket,
} from "@mako-cloud/management-sdk";

import type { CommandContext } from "../cli/context.js";
import { CliError, EXIT, usageError } from "../cli/errors.js";
import { CLI_NAME } from "../cli/name.js";
import type { TableColumn } from "../cli/output.js";
import type { Command } from "../cli/registry.js";
import { PROJECT_OPTION, projectFrom } from "./shared.js";

/**
 * Promotion copies an application's shape from one environment to another --
 * collection schemas, indexes, the active document policy of each collection,
 * and storage bucket settings -- so what was built and tested in Development
 * can be brought to Production without re-typing it. It never copies data,
 * secrets, keys, webhooks, sign-in settings, allowed origins, or domains:
 * those differ between environments by design.
 *
 * It plans first and changes nothing unless `--apply` is given. A step that
 * needs a person -- a schema change that needs a migration, a target that is
 * ahead of the source, function code -- is reported with what to do.
 */

export type PromotionStepKind =
  | "create_collection"
  | "publish_schema"
  | "create_index"
  | "activate_policy"
  | "create_bucket"
  | "update_bucket"
  | "attention";

export interface PromotionStep {
  readonly kind: PromotionStepKind;
  readonly resource: string;
  readonly detail: string;
}

interface PlannedStep extends PromotionStep {
  readonly apply?: () => Promise<string>;
}

export interface EnvironmentShape {
  readonly collections: readonly Collection[];
  readonly indexes: ReadonlyMap<string, readonly CollectionIndex[]>;
  readonly policies: ReadonlyMap<string, PolicyShape | null>;
  readonly buckets: readonly StorageBucket[];
  readonly functions: readonly FunctionShape[];
  /** What promotion does not copy but a release usually needs; compared only when read. */
  readonly settings?: SettingsShape;
}

/**
 * Environment settings promotion leaves alone. They are environment-specific
 * (redirect URLs, origins, endpoints) or cannot be read back (secret
 * values), and copying sign-in settings could weaken a target. A target that
 * lacks what the source has is reported, never changed.
 */
export interface SettingsShape {
  readonly emailVerificationRequired: boolean;
  readonly magicLinksEnabled: boolean;
  readonly redirectUrls: readonly string[];
  readonly providers: readonly string[];
  readonly allowedOrigins: readonly string[];
  readonly signingKey: boolean;
  /** Customized templates only: kind to their subject and body. */
  readonly templates: ReadonlyMap<string, string>;
  readonly webhooks: readonly string[];
  /** The secrets the source's functions attach that exist here. */
  readonly secrets: ReadonlySet<string>;
  /** Schedule names per function. */
  readonly schedules: ReadonlyMap<string, readonly string[]>;
}

export interface PolicyShape {
  readonly version: number;
  readonly rules: readonly unknown[];
}

export interface FunctionShape {
  readonly name: string;
  readonly state: string;
  readonly activeVersion: number | null;
  readonly configuration: unknown;
}

/** JSON with object keys in order, so equal shapes compare equal. */
export function canonical(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(canonical).join(",")}]`;
  if (value !== null && typeof value === "object") {
    return `{${Object.keys(value as Record<string, unknown>)
      .sort()
      .map((key) => `${JSON.stringify(key)}:${canonical((value as Record<string, unknown>)[key])}`)
      .join(",")}}`;
  }
  return JSON.stringify(value);
}

const bucketSettings = (bucket: StorageBucket) =>
  canonical({
    access: bucket.access,
    maxObjectBytes: bucket.maxObjectBytes,
    allowedContentTypes: [...bucket.allowedContentTypes].sort(),
    rules: bucket.rules,
  });

/**
 * The part of an environment to promote: only the named collections and
 * buckets when any are named, everything otherwise. Development gathers
 * experiments; a release brings over what it means to.
 */
export function selectShape(
  shape: EnvironmentShape,
  collections: readonly string[],
  buckets: readonly string[],
): EnvironmentShape {
  if (collections.length === 0 && buckets.length === 0) return shape;
  const wanted = new Set(collections);
  const wantedBuckets = new Set(buckets);
  return {
    collections: shape.collections.filter((item) => wanted.has(item.id)),
    indexes: shape.indexes,
    policies: shape.policies,
    buckets: shape.buckets.filter((item) => wantedBuckets.has(item.id)),
    functions: [],
  };
}

const count = (n: number, one: string) => `${n} ${one}${n === 1 ? "" : "s"}`;

/** The attention rows for settings the target lacks or has differently. */
export function planSettings(
  source: SettingsShape,
  target: SettingsShape,
  functions: readonly FunctionShape[],
): readonly PromotionStep[] {
  const steps: PromotionStep[] = [];
  const attention = (resource: string, detail: string) =>
    steps.push({ kind: "attention", resource, detail });
  const signIn: string[] = [];
  if (source.emailVerificationRequired !== target.emailVerificationRequired) {
    signIn.push(
      `email verification is ${source.emailVerificationRequired ? "required" : "off"} in the source and ${target.emailVerificationRequired ? "required" : "off"} in the target`,
    );
  }
  if (source.magicLinksEnabled !== target.magicLinksEnabled) {
    signIn.push(
      `magic links are ${source.magicLinksEnabled ? "on" : "off"} in the source and ${target.magicLinksEnabled ? "on" : "off"} in the target`,
    );
  }
  if (source.redirectUrls.length > 0 && target.redirectUrls.length === 0) {
    signIn.push(
      `the target allows no redirect URLs (the source allows ${source.redirectUrls.length})`,
    );
  }
  const missingProviders = source.providers.filter((name) => !target.providers.includes(name));
  if (missingProviders.length > 0) {
    signIn.push(`providers missing in the target: ${missingProviders.join(", ")}`);
  }
  if (signIn.length > 0) {
    attention(
      "sign-in settings",
      `${signIn.join("; ")}; review with ${CLI_NAME} auth-settings get and set them with ${CLI_NAME} auth-settings set --env <target>`,
    );
  }
  if (source.signingKey && !target.signingKey) {
    attention(
      "signing key",
      `the target has no JWT signing key, so application users cannot sign in there; create one with ${CLI_NAME} keys signing init --env <target>`,
    );
  }
  if (
    source.allowedOrigins.length > 0 &&
    canonical([...source.allowedOrigins].sort()) !== canonical([...target.allowedOrigins].sort())
  ) {
    attention(
      "allowed origins",
      `the source allows ${count(source.allowedOrigins.length, "origin")} and the target ${target.allowedOrigins.length === 0 ? "none" : count(target.allowedOrigins.length, "origin")}; set the target's own with ${CLI_NAME} allowed-origins set --origin <url> --env <target>`,
    );
  }
  const templates = [...source.templates].filter(
    ([kind, body]) => target.templates.get(kind) !== body,
  );
  if (templates.length > 0) {
    attention(
      "email templates",
      `customized in the source and not the same in the target: ${templates.map(([kind]) => kind).join(", ")}; copy them with ${CLI_NAME} email-templates get <kind> and email-templates set <kind> --env <target>`,
    );
  }
  const webhooks = source.webhooks.filter((hook) => !target.webhooks.includes(hook));
  if (webhooks.length > 0) {
    attention(
      "webhooks",
      `${count(webhooks.length, "webhook endpoint")} of the source ${webhooks.length === 1 ? "is" : "are"} not registered in the target; register the target's own with ${CLI_NAME} webhooks create --env <target>`,
    );
  }
  for (const item of functions) {
    if (item.state === "deleted") continue;
    const names = secretNamesOf(item);
    const missing = names.filter((name) => !target.secrets.has(name));
    if (missing.length > 0) {
      attention(
        `function ${item.name}`,
        `attaches ${missing.join(", ")}, missing in the target; create ${missing.length === 1 ? "it" : "them"} with ${CLI_NAME} functions secrets create <name> --env <target>`,
      );
    }
    const wanted = source.schedules.get(item.name) ?? [];
    const present = target.schedules.get(item.name) ?? [];
    const unscheduled = wanted.filter((name) => !present.includes(name));
    if (unscheduled.length > 0) {
      attention(
        `function ${item.name}`,
        `schedules missing in the target: ${unscheduled.join(", ")}; add them with ${CLI_NAME} schedules create --function ${item.name} --env <target>`,
      );
    }
  }
  return steps;
}

/** The secret names a function's configuration attaches. */
export function secretNamesOf(item: FunctionShape): readonly string[] {
  const configuration = item.configuration as { readonly secretNames?: readonly string[] } | null;
  return configuration?.secretNames ?? [];
}

/** What promoting `source` onto `target` would change, in the order to change it. */
export function planPromotion(
  source: EnvironmentShape,
  target: EnvironmentShape,
): readonly PromotionStep[] {
  const steps: PromotionStep[] = [];
  const targetCollections = new Map(target.collections.map((item) => [item.id, item]));
  for (const collection of source.collections) {
    if (collection.state !== "active") continue;
    const existing = targetCollections.get(collection.id);
    if (existing === undefined) {
      steps.push({
        kind: "create_collection",
        resource: collection.id,
        detail: `create at schema v${collection.schemaVersion}`,
      });
    } else if (existing.schemaVersion < collection.schemaVersion) {
      steps.push({
        kind: "publish_schema",
        resource: collection.id,
        detail: `publish schema v${collection.schemaVersion} over v${existing.schemaVersion}`,
      });
    } else if (existing.schemaVersion > collection.schemaVersion) {
      steps.push({
        kind: "attention",
        resource: collection.id,
        detail: `target is at schema v${existing.schemaVersion}, ahead of the source's v${collection.schemaVersion}; left as it is`,
      });
    } else if (canonical(existing.jsonSchema) !== canonical(collection.jsonSchema)) {
      steps.push({
        kind: "attention",
        resource: collection.id,
        detail: `both are at schema v${collection.schemaVersion} but the schemas differ; publish a new version in the source`,
      });
    }
    const targetIndexes = new Set(
      (target.indexes.get(collection.id) ?? []).map((index) => `${index.name}@${index.version}`),
    );
    for (const index of source.indexes.get(collection.id) ?? []) {
      if (index.state === "failed" || targetIndexes.has(`${index.name}@${index.version}`)) continue;
      steps.push({
        kind: "create_index",
        resource: `${collection.id}/${index.name}`,
        detail: `create index ${index.name} v${index.version} (${index.kind})`,
      });
    }
    const policy = source.policies.get(collection.id) ?? null;
    const current = target.policies.get(collection.id) ?? null;
    if (policy !== null && canonical(policy.rules) !== canonical(current?.rules ?? null)) {
      steps.push({
        kind: "activate_policy",
        resource: collection.id,
        detail: `activate the source's policy (${policy.rules.length} rules)${current === null ? "; the target has none, so it denies everything" : ""}`,
      });
    }
  }
  const targetBuckets = new Map(target.buckets.map((bucket) => [bucket.id, bucket]));
  for (const bucket of source.buckets) {
    const existing = targetBuckets.get(bucket.id);
    if (existing === undefined) {
      steps.push({
        kind: "create_bucket",
        resource: bucket.id,
        detail: `create (${bucket.access})`,
      });
    } else if (bucketSettings(existing) !== bucketSettings(bucket)) {
      steps.push({
        kind: "update_bucket",
        resource: bucket.id,
        detail: "update access, limits, and rules",
      });
    }
  }
  const targetFunctions = new Map(target.functions.map((item) => [item.name, item]));
  for (const item of source.functions) {
    if (item.state === "deleted") continue;
    const existing = targetFunctions.get(item.name);
    const missing = existing === undefined || existing.state === "deleted";
    const undeployed = !missing && item.activeVersion !== null && existing.activeVersion === null;
    const reconfigured =
      !missing && canonical(existing.configuration) !== canonical(item.configuration);
    if (missing || undeployed || reconfigured) {
      steps.push({
        kind: "attention",
        resource: `function ${item.name}`,
        detail: `${missing ? "missing in the target" : undeployed ? "not deployed in the target" : "configured differently in the target"}; deploy it from its source with ${CLI_NAME} functions deploy <dir> --name ${item.name}${missing ? " --create" : ""} --env <target>`,
      });
    }
  }
  if (source.settings !== undefined && target.settings !== undefined) {
    steps.push(...planSettings(source.settings, target.settings, source.functions));
  }
  return steps;
}

async function readSettings(
  client: MakoManagementClient,
  projectId: string,
  environmentId: string,
  functions: readonly FunctionShape[],
  secretNames: readonly string[],
): Promise<SettingsShape> {
  const [auth, origins, keys, templates, webhooks] = await Promise.all([
    client.getAuthSettings(projectId, environmentId),
    client.getAllowedOrigins(projectId, environmentId),
    client.listJwtSigningKeys(projectId, environmentId),
    client.listEmailTemplates(projectId, environmentId),
    client.listWebhookEndpoints(projectId, environmentId),
  ]);
  const secrets = new Set<string>();
  for (const name of new Set(secretNames)) {
    const found = await client.getFunctionSecret(projectId, environmentId, name).then(
      (secret) => secret.state !== "retired",
      () => false,
    );
    if (found) secrets.add(name);
  }
  const schedules = new Map<string, readonly string[]>();
  for (const item of functions) {
    if (item.state === "deleted") continue;
    const list = await client
      .listFunctionSchedules(projectId, environmentId, item.name)
      .catch(() => []);
    schedules.set(
      item.name,
      list.map((schedule) => schedule.name),
    );
  }
  return {
    emailVerificationRequired: auth.emailVerification.required,
    magicLinksEnabled: auth.magicLinks.enabled,
    redirectUrls: auth.redirectUrls,
    providers: auth.providers.map((provider) => provider.name),
    allowedOrigins: origins.allowedOrigins,
    signingKey: keys.some((key) => key.state === "active"),
    templates: new Map(
      templates
        .filter((template) => !template.isDefault)
        .map((template) => [
          template.kind,
          canonical({ subject: template.subject, body: template.textBody }),
        ]),
    ),
    webhooks: webhooks.map((hook) =>
      canonical({ url: hook.url, subscriptions: hook.subscriptions }),
    ),
    secrets,
    schedules,
  };
}

async function readShape(
  client: MakoManagementClient,
  projectId: string,
  environmentId: string,
  secretNames?: readonly string[],
): Promise<EnvironmentShape> {
  const collections = await client.listCollections(projectId, environmentId);
  const indexes = new Map<string, readonly CollectionIndex[]>();
  const policies = new Map<string, PolicyShape | null>();
  for (const collection of collections) {
    if (collection.state !== "active") continue;
    indexes.set(
      collection.id,
      await client.listCollectionIndexes(projectId, environmentId, collection.id),
    );
    const active = await client.getActiveCollectionPolicy(projectId, environmentId, collection.id);
    policies.set(
      collection.id,
      active.policy === undefined || active.policy === null
        ? null
        : { version: active.policy.version, rules: active.policy.rules },
    );
  }
  const buckets = await client.listStorageBuckets(projectId, environmentId);
  const functions = (await client.listFunctions(projectId, environmentId)).map((item) => ({
    name: item.name,
    state: item.state,
    activeVersion: item.activeVersion ?? null,
    configuration: item.configuration,
  }));
  const settings = await readSettings(
    client,
    projectId,
    environmentId,
    functions,
    secretNames ?? functions.flatMap(secretNamesOf),
  );
  return { collections, indexes, policies, buckets, functions, settings };
}

/** A collection is created asynchronously; indexes and policies need it active. */
async function waitForActive(
  client: MakoManagementClient,
  projectId: string,
  environmentId: string,
  collectionId: string,
): Promise<void> {
  const deadline = Date.now() + 60_000;
  for (;;) {
    const collection = await client.getCollection(projectId, environmentId, collectionId);
    if (collection.state === "active") return;
    if (Date.now() > deadline) {
      throw new Error(`collection ${collectionId} did not become active within 60 seconds`);
    }
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
}

function withActions(
  context: CommandContext,
  client: MakoManagementClient,
  projectId: string,
  source: EnvironmentShape,
  target: EnvironmentShape,
  targetId: string,
  steps: readonly PromotionStep[],
): readonly PlannedStep[] {
  const collections = new Map(source.collections.map((item) => [item.id, item]));
  const buckets = new Map(source.buckets.map((item) => [item.id, item]));
  return steps.map((step): PlannedStep => {
    const [collectionId = step.resource, indexName] = step.resource.split("/");
    const collection = collections.get(collectionId);
    switch (step.kind) {
      case "create_collection":
        return {
          ...step,
          apply: async () => {
            if (collection === undefined) throw new Error("source collection vanished");
            await client.createCollection(
              projectId,
              targetId,
              {
                id: collection.id,
                schemaVersion: collection.schemaVersion,
                jsonSchema: collection.jsonSchema,
                primaryKey: collection.primaryKey,
              },
              context.idempotencyKey(),
            );
            await waitForActive(client, projectId, targetId, collection.id);
            return "created";
          },
        };
      case "publish_schema":
        return {
          ...step,
          apply: async () => {
            if (collection === undefined) throw new Error("source collection vanished");
            const result = await client.publishCollectionSchema(
              projectId,
              targetId,
              collection.id,
              {
                schemaVersion: collection.schemaVersion,
                jsonSchema: collection.jsonSchema,
                primaryKey: collection.primaryKey,
              },
              context.idempotencyKey(),
            );
            if (result.status === "published") return "published";
            throw new Error(
              `needs a migration in the target: ${(result.compatibility?.issues ?? []).join("; ")}`,
            );
          },
        };
      case "create_index":
        return {
          ...step,
          apply: async () => {
            const index = source.indexes.get(collectionId)?.find((item) => item.name === indexName);
            if (index === undefined) throw new Error("source index vanished");
            await client.createCollectionIndex(
              projectId,
              targetId,
              collectionId,
              { name: index.name, version: index.version, kind: index.kind, fields: index.fields },
              context.idempotencyKey(),
            );
            return "created";
          },
        };
      case "activate_policy":
        return {
          ...step,
          apply: async () => {
            const policy = source.policies.get(collectionId);
            if (policy === undefined || policy === null) throw new Error("source policy vanished");
            // A new version above both sides; a draft already holding that
            // number moves it up, a few times at most.
            let version =
              Math.max(policy.version, target.policies.get(collectionId)?.version ?? 0) + 1;
            for (let attempt = 0; ; attempt += 1) {
              try {
                await client.createCollectionPolicyDraft(
                  projectId,
                  targetId,
                  collectionId,
                  { version, rules: policy.rules as never },
                  context.idempotencyKey(),
                );
                break;
              } catch (error) {
                if (attempt >= 5) throw error;
                version += 1;
              }
            }
            const validation = await client.validateCollectionPolicy(
              projectId,
              targetId,
              collectionId,
              version,
            );
            if (!validation.valid) throw new Error(`policy v${version} does not validate`);
            await client.activateCollectionPolicy(
              projectId,
              targetId,
              collectionId,
              version,
              context.idempotencyKey(),
            );
            return `activated as v${version}`;
          },
        };
      case "create_bucket":
      case "update_bucket":
        return {
          ...step,
          apply: async () => {
            const bucket = buckets.get(step.resource);
            if (bucket === undefined) throw new Error("source bucket vanished");
            const settings = {
              access: bucket.access,
              maxObjectBytes: bucket.maxObjectBytes,
              allowedContentTypes: bucket.allowedContentTypes,
              rules: bucket.rules,
            };
            if (step.kind === "create_bucket") {
              await client.createStorageBucket(
                projectId,
                targetId,
                { id: bucket.id, ...settings },
                context.idempotencyKey(),
              );
              return "created";
            }
            await client.updateStorageBucket(
              projectId,
              targetId,
              bucket.id,
              settings,
              context.idempotencyKey(),
            );
            return "updated";
          },
        };
      default:
        return step;
    }
  });
}

const PLAN_COLUMNS: readonly TableColumn[] = [
  { key: "kind" },
  { key: "resource" },
  { key: "detail" },
];
const RESULT_COLUMNS: readonly TableColumn[] = [...PLAN_COLUMNS, { key: "result" }];

export const promoteCommands: readonly Command[] = [
  {
    path: ["envs", "promote"],
    summary:
      "Copy collections, indexes, policies, and buckets from one environment to another (plans unless --apply)",
    operations: [
      "getEnvironment",
      "listCollections",
      "getCollection",
      "createCollection",
      "publishCollectionSchema",
      "listCollectionIndexes",
      "createCollectionIndex",
      "getActiveCollectionPolicy",
      "createCollectionPolicyDraft",
      "validateCollectionPolicy",
      "activateCollectionPolicy",
      "listStorageBuckets",
      "createStorageBucket",
      "updateStorageBucket",
      "listFunctions",
    ],
    options: {
      ...PROJECT_OPTION,
      from: {
        type: "string",
        description: "Environment to copy from, such as Development",
        placeholder: "<env-id>",
        required: true,
      },
      to: {
        type: "string",
        description: "Environment to bring up to date, such as Production",
        placeholder: "<env-id>",
        required: true,
      },
      collection: {
        type: "string",
        multiple: true,
        description: "Promote only this collection (repeatable); with none named, all of them",
        placeholder: "<collection-id>",
      },
      bucket: {
        type: "string",
        multiple: true,
        description: "Promote only this bucket (repeatable); with none named, all of them",
        placeholder: "<bucket-id>",
      },
      apply: {
        type: "boolean",
        description: "Make the changes; without it the plan is only shown",
      },
    },
    run: async (context, args) => {
      const projectId = projectFrom(context, args);
      const from = args.string("from");
      const to = args.string("to");
      if (from === undefined || to === undefined) {
        throw usageError("--from <env-id> and --to <env-id> are required");
      }
      if (from === to) throw usageError("--from and --to name the same environment");
      const client = await context.management();
      await client.getEnvironment(projectId, from);
      await client.getEnvironment(projectId, to);
      const everything = await readShape(client, projectId, from);
      // The target is asked about the secrets the source's functions need.
      const target = await readShape(
        client,
        projectId,
        to,
        everything.functions.flatMap(secretNamesOf),
      );
      const named = args.strings("collection");
      const missing = named.filter((id) => !everything.collections.some((item) => item.id === id));
      if (missing.length > 0) {
        throw usageError(`${from} has no collection ${missing.join(", ")}`);
      }
      const source = selectShape(everything, named, args.strings("bucket"));
      const steps = withActions(
        context,
        client,
        projectId,
        source,
        target,
        to,
        planPromotion(source, target),
      );
      const selectors = [
        ...named.map((id) => ` --collection ${id}`),
        ...args.strings("bucket").map((id) => ` --bucket ${id}`),
      ].join("");
      if (!args.boolean("apply")) {
        context.out(
          steps.map(({ kind, resource, detail }) => ({ kind, resource, detail })),
          { columns: PLAN_COLUMNS },
        );
        if (!context.json) {
          context.info(
            steps.some((step) => step.apply !== undefined)
              ? `Nothing was changed. Apply with: ${CLI_NAME} envs promote --from ${from} --to ${to} --project ${projectId}${selectors} --apply`
              : `${to} already matches ${from} in everything promotion copies.`,
          );
        }
        return;
      }
      const results: Array<PromotionStep & { result: string }> = [];
      let failed = false;
      for (const step of steps) {
        const { kind, resource, detail } = step;
        if (step.apply === undefined) {
          results.push({ kind, resource, detail, result: "needs you" });
          continue;
        }
        try {
          results.push({ kind, resource, detail, result: await step.apply() });
        } catch (error) {
          failed = true;
          results.push({
            kind,
            resource,
            detail,
            result: `failed: ${error instanceof Error ? error.message : String(error)}`,
          });
        }
      }
      context.out(results, { columns: RESULT_COLUMNS });
      if (failed) {
        throw new CliError(
          "some promotion steps failed; see the results above",
          EXIT.api,
          "CLI_PROMOTION_FAILED",
        );
      }
    },
  },
];
