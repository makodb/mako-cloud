import type {
  WebhookDeliveryState,
  WebhookEndpointCreate,
  WebhookEndpointUpdate,
  WebhookEvent,
  WebhookSubscription,
} from "@mako-cloud/management-sdk";

import type { CommandContext } from "../cli/context.js";
import { usageError } from "../cli/errors.js";
import type { TableColumn } from "../cli/output.js";
import type { Command, CommandArgs, OptionSpec, PositionalSpec } from "../cli/registry.js";
import { SECRET_FILE_OPTION, TENANT_OPTIONS, tenantFrom } from "./shared.js";

const EVENTS: readonly WebhookEvent[] = ["insert", "update", "delete"];
const DELIVERY_STATES: readonly WebhookDeliveryState[] = ["pending", "delivered", "failed"];
const COLLECTION_ID = /^[a-z][a-z0-9_-]{0,62}$/u;
/** The API's ceiling on one page of deliveries. */
const MAX_LIST_LIMIT = 200;

const ENDPOINT_COLUMNS: readonly TableColumn[] = [
  { key: "id" },
  { key: "url" },
  { key: "state" },
  { key: "consecutiveFailures", label: "failures" },
  { key: "secretVersion" },
  { key: "updatedAt" },
];

const DELIVERY_COLUMNS: readonly TableColumn[] = [
  { key: "id" },
  { key: "event" },
  { key: "collectionId" },
  { key: "documentId" },
  { key: "state" },
  { key: "attempts" },
  { key: "lastResponseStatus", label: "status" },
  { key: "lastError", label: "error" },
  { key: "createdAt" },
];

const WEBHOOK_ID: PositionalSpec = {
  name: "webhook-id",
  description: "Webhook endpoint id (whk_…)",
  required: true,
};

const DELIVERY_ID: PositionalSpec = {
  name: "delivery-id",
  description: "Delivery id (whd_…)",
  required: true,
};

/** Options `webhooks create` and `webhooks update` share: what the endpoint is. */
const ENDPOINT_OPTIONS: Readonly<Record<string, OptionSpec>> = {
  url: {
    type: "string",
    description: "Absolute https URL the deliveries are posted to",
    placeholder: "<url>",
  },
  subscribe: {
    type: "string",
    multiple: true,
    description:
      "Collection and the events to deliver, e.g. orders:insert,update; repeat for several collections, omit the events for all three",
    placeholder: "<collection>[:<events>]",
  },
  description: {
    type: "string",
    description: "A note for people; not part of any delivery",
    placeholder: "<text>",
  },
};

function webhookOf(args: CommandArgs): string {
  return args.requirePositional(0, "webhook-id");
}

function deliveryOf(args: CommandArgs): string {
  return args.requirePositional(1, "delivery-id");
}

function urlFrom(args: CommandArgs): string | undefined {
  const url = args.string("url");
  if (url === undefined) return undefined;
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    throw usageError("--url must be an absolute URL");
  }
  if (parsed.protocol !== "https:" && parsed.protocol !== "http:") {
    throw usageError("--url must use https (http is admitted only to loopback outside production)");
  }
  return url;
}

function isEvent(value: string): value is WebhookEvent {
  return EVENTS.includes(value as WebhookEvent);
}

/**
 * `--subscribe <collection>[:<events>]`, merged per collection so a collection
 * named twice subscribes to the union of its events, in the order given.
 */
function subscriptionsFrom(args: CommandArgs): WebhookSubscription[] | undefined {
  if (args.values.subscribe === undefined) return undefined;
  const byCollection = new Map<string, WebhookEvent[]>();
  for (const spec of args.strings("subscribe")) {
    const separator = spec.indexOf(":");
    const collectionId = (separator === -1 ? spec : spec.slice(0, separator)).trim();
    if (!COLLECTION_ID.test(collectionId)) {
      throw usageError(`--subscribe must name a collection id, got "${spec}"`);
    }
    const events =
      separator === -1
        ? [...EVENTS]
        : spec
            .slice(separator + 1)
            .split(",")
            .map((event) => event.trim())
            .filter((event) => event !== "");
    if (events.length === 0) {
      throw usageError(`--subscribe ${collectionId}: list at least one of ${EVENTS.join(", ")}`);
    }
    const merged = byCollection.get(collectionId) ?? [];
    for (const event of events) {
      if (!isEvent(event)) {
        throw usageError(
          `--subscribe ${collectionId}: unknown event "${event}"; use ${EVENTS.join(", ")}`,
        );
      }
      if (!merged.includes(event)) merged.push(event);
    }
    byCollection.set(collectionId, merged);
  }
  return Array.from(byCollection, ([collectionId, events]) => ({ collectionId, events }));
}

function deliveryStateFrom(args: CommandArgs): WebhookDeliveryState | undefined {
  const state = args.string("state");
  if (state === undefined) return undefined;
  if (!DELIVERY_STATES.includes(state as WebhookDeliveryState)) {
    throw usageError(`--state must be one of ${DELIVERY_STATES.join(", ")}`);
  }
  return state as WebhookDeliveryState;
}

/**
 * Prints the endpoint, then its signing secret exactly once: the API stores
 * it sealed and never returns it again, so the warning goes out before it.
 */
async function shownOnce(
  context: CommandContext,
  args: CommandArgs,
  issue: { readonly endpoint: { readonly id: string }; readonly signingSecret: string },
): Promise<void> {
  context.info(
    `the signing secret of webhook ${issue.endpoint.id} is shown once and cannot be read back; store it now`,
  );
  await context.secret(
    `signing secret for webhook ${issue.endpoint.id}`,
    issue.signingSecret,
    { ...issue.endpoint },
    args.string("secret-file"),
  );
}

async function listEndpoints(context: CommandContext, args: CommandArgs): Promise<void> {
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  const endpoints = await client.listWebhookEndpoints(projectId, environmentId);
  context.out(endpoints, { columns: ENDPOINT_COLUMNS });
}

async function getEndpoint(context: CommandContext, args: CommandArgs): Promise<void> {
  const webhookId = webhookOf(args);
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  context.out(await client.getWebhookEndpoint(projectId, environmentId, webhookId));
}

async function createEndpoint(context: CommandContext, args: CommandArgs): Promise<void> {
  const { projectId, environmentId } = tenantFrom(context, args);
  const url = urlFrom(args);
  if (url === undefined) throw usageError("--url <url> is required");
  const subscriptions = subscriptionsFrom(args);
  if (subscriptions === undefined) {
    throw usageError("--subscribe <collection>[:<events>] is required at least once");
  }
  const description = args.string("description");
  const request: WebhookEndpointCreate = {
    url,
    subscriptions,
    enabled: !args.boolean("disabled"),
    ...(description !== undefined ? { description } : {}),
  };
  const client = await context.management();
  const issue = await client.createWebhookEndpoint(
    projectId,
    environmentId,
    request,
    context.idempotencyKey(),
  );
  await shownOnce(context, args, issue);
}

async function updateEndpoint(context: CommandContext, args: CommandArgs): Promise<void> {
  const webhookId = webhookOf(args);
  const { projectId, environmentId } = tenantFrom(context, args);
  const enable = args.boolean("enable");
  const disable = args.boolean("disable");
  if (enable && disable) throw usageError("--enable and --disable exclude each other");
  const url = urlFrom(args);
  const subscriptions = subscriptionsFrom(args);
  const description = args.string("description");
  // Only what was given is sent; the API leaves the rest as it is.
  const request: WebhookEndpointUpdate = {
    ...(url !== undefined ? { url } : {}),
    ...(subscriptions !== undefined ? { subscriptions } : {}),
    ...(description !== undefined ? { description } : {}),
    ...(enable || disable ? { enabled: enable } : {}),
  };
  if (Object.keys(request).length === 0) {
    throw usageError(
      "nothing to update: pass at least one of --url, --subscribe, --description, --enable, --disable",
    );
  }
  const client = await context.management();
  const endpoint = await client.updateWebhookEndpoint(
    projectId,
    environmentId,
    webhookId,
    request,
    context.idempotencyKey(),
  );
  context.out(endpoint);
}

async function deleteEndpoint(context: CommandContext, args: CommandArgs): Promise<void> {
  const webhookId = webhookOf(args);
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  await client.deleteWebhookEndpoint(projectId, environmentId, webhookId, context.idempotencyKey());
  if (context.json) context.out({ id: webhookId, state: "deleted" });
  else context.info(`webhook ${webhookId} deleted`);
}

async function rotateSecret(context: CommandContext, args: CommandArgs): Promise<void> {
  const webhookId = webhookOf(args);
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  const issue = await client.rotateWebhookSecret(
    projectId,
    environmentId,
    webhookId,
    context.idempotencyKey(),
  );
  await shownOnce(context, args, issue);
}

async function resumeEndpoint(context: CommandContext, args: CommandArgs): Promise<void> {
  const webhookId = webhookOf(args);
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  context.out(
    await client.resumeWebhookEndpoint(
      projectId,
      environmentId,
      webhookId,
      context.idempotencyKey(),
    ),
  );
}

async function listDeliveries(context: CommandContext, args: CommandArgs): Promise<void> {
  const webhookId = webhookOf(args);
  const { projectId, environmentId } = tenantFrom(context, args);
  const state = deliveryStateFrom(args);
  const limit = args.integer("limit");
  if (limit !== undefined && (limit < 1 || limit > MAX_LIST_LIMIT)) {
    throw usageError(`--limit must be between 1 and ${MAX_LIST_LIMIT}`);
  }
  const start = args.string("cursor");
  const client = await context.management();
  const page = await context.collect((cursor) => {
    const next = cursor ?? start;
    return client.listWebhookDeliveries(projectId, environmentId, webhookId, {
      ...(state !== undefined ? { state } : {}),
      ...(limit !== undefined ? { limit } : {}),
      ...(next !== undefined ? { cursor: next } : {}),
    });
  });
  context.out(page, { columns: DELIVERY_COLUMNS });
}

async function redeliver(context: CommandContext, args: CommandArgs): Promise<void> {
  const webhookId = webhookOf(args);
  const deliveryId = deliveryOf(args);
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  context.out(
    await client.redeliverWebhookDelivery(
      projectId,
      environmentId,
      webhookId,
      deliveryId,
      context.idempotencyKey(),
    ),
  );
}

export const webhooksCommands: readonly Command[] = [
  {
    path: ["webhooks", "list"],
    summary: "List the environment's webhook endpoints with their state and failure counts",
    operations: ["listWebhookEndpoints"],
    options: { ...TENANT_OPTIONS },
    run: listEndpoints,
  },
  {
    path: ["webhooks", "get"],
    summary: "Show a webhook endpoint, its subscriptions, and why it is paused if it is",
    operations: ["getWebhookEndpoint"],
    positionals: [WEBHOOK_ID],
    options: { ...TENANT_OPTIONS },
    run: getEndpoint,
  },
  {
    path: ["webhooks", "create"],
    summary: "Register a webhook endpoint for collection events; the signing secret is shown once",
    operations: ["createWebhookEndpoint"],
    options: {
      ...TENANT_OPTIONS,
      ...ENDPOINT_OPTIONS,
      disabled: {
        type: "boolean",
        description: "Register the endpoint without delivering to it until it is enabled",
      },
      ...SECRET_FILE_OPTION,
    },
    run: createEndpoint,
  },
  {
    path: ["webhooks", "update"],
    summary:
      "Change a webhook endpoint's URL, subscriptions, description, or enabled flag; only given options are sent",
    operations: ["updateWebhookEndpoint"],
    positionals: [WEBHOOK_ID],
    options: {
      ...TENANT_OPTIONS,
      ...ENDPOINT_OPTIONS,
      enable: { type: "boolean", description: "Queue deliveries again" },
      disable: { type: "boolean", description: "Stop queueing deliveries" },
    },
    run: updateEndpoint,
  },
  {
    path: ["webhooks", "delete"],
    summary: "Remove a webhook endpoint with its subscriptions and delivery log",
    operations: ["deleteWebhookEndpoint"],
    positionals: [WEBHOOK_ID],
    options: { ...TENANT_OPTIONS },
    destructive: { action: "delete webhook", resource: webhookOf },
    run: deleteEndpoint,
  },
  {
    path: ["webhooks", "rotate-secret"],
    summary:
      "Replace a webhook endpoint's signing secret; the new one is shown once and the old one stops signing at once",
    operations: ["rotateWebhookSecret"],
    positionals: [WEBHOOK_ID],
    options: { ...TENANT_OPTIONS, ...SECRET_FILE_OPTION },
    destructive: { action: "rotate the signing secret of webhook", resource: webhookOf },
    run: rotateSecret,
  },
  {
    path: ["webhooks", "resume"],
    summary: "Resume a webhook endpoint the platform paused after sustained failure",
    operations: ["resumeWebhookEndpoint"],
    positionals: [WEBHOOK_ID],
    options: { ...TENANT_OPTIONS },
    run: resumeEndpoint,
  },
  {
    path: ["webhooks", "deliveries"],
    summary:
      "List a webhook endpoint's recent deliveries, newest first; --all follows the cursor to the end",
    operations: ["listWebhookDeliveries"],
    positionals: [WEBHOOK_ID],
    options: {
      ...TENANT_OPTIONS,
      state: {
        type: "string",
        description: "Only deliveries in this state",
        placeholder: `<${DELIVERY_STATES.join("|")}>`,
      },
      limit: {
        type: "string",
        description: `Deliveries per page, 1-${MAX_LIST_LIMIT} (default: 50)`,
        placeholder: "<n>",
      },
      cursor: {
        type: "string",
        description: "Continue from a previous page's next cursor",
        placeholder: "<cursor>",
      },
    },
    run: listDeliveries,
  },
  {
    path: ["webhooks", "redeliver"],
    summary: "Queue a new signed delivery of one event, logged as a redelivery of the original",
    operations: ["redeliverWebhookDelivery"],
    positionals: [WEBHOOK_ID, DELIVERY_ID],
    options: { ...TENANT_OPTIONS },
    run: redeliver,
  },
];
