/**
 * Rational's `institution-sync` function: the scheduled half of an account
 * that syncs itself.
 *
 * Routes (JSON in and out):
 *   POST /sync                  every connected account, one statement each
 *   GET  /institution/statement the simulated bank, called over HTTP like a
 *                               real one would be
 *
 * The schedule invokes `/sync` every fifteen minutes. What matters is not the
 * bank -- that one is simulated and deterministic -- but everything around
 * it: a schedule that fires with nobody signed in, a function writing with a
 * service credential, and transactions that must not double when a sync runs
 * twice over an overlapping window. Idempotence is by `(account,
 * external_id)`, which is also the transaction's document id, so a repeated
 * entry is an update of the same document rather than a second one.
 */
import {
  createServiceClient,
  type FunctionDocument,
  type JsonObject,
  type ServiceFunctionClient,
} from "@mako-cloud/edge-sdk";

import { serviceCredential } from "./credential.ts";
import { statement } from "./institution.ts";

declare const Deno: { readonly env: { get(name: string): string | undefined } };

/** The schema version of `mako/collections.json`. */
const SCHEMA_VERSION = 1;
const CONNECTIONS = "connections";
const TRANSACTIONS = "transactions";
/** How far back each sync asks. Overlap is deliberate: it is what proves the
 * idempotence, and it is what a real institution needs anyway because entries
 * settle days after they happen. */
const WINDOW_DAYS = 10;

export default {
  async fetch(request: Request): Promise<Response> {
    const url = new URL(request.url);
    const route =
      url.pathname
        .split("/")
        .filter((part) => part !== "")
        .pop() ?? "";
    try {
      if (route === "statement") return institutionRoute(url);
      if (request.method !== "POST") {
        return failure(405, "invalid_request", "institution-sync routes are POST");
      }
      if (route === "sync") return await sync(request);
      return failure(404, "not_found", `unknown institution-sync route ${route}`);
    } catch (error) {
      if (error instanceof RouteError) return failure(error.status, error.code, error.message);
      return failure(500, "internal", "the institution-sync function failed");
    }
  },
};

/** The simulated bank, on its own route. */
function institutionRoute(url: URL): Response {
  const account = url.searchParams.get("account") ?? "";
  const through = url.searchParams.get("through") ?? today();
  const days = Number(url.searchParams.get("days") ?? WINDOW_DAYS);
  if (account === "") return failure(400, "invalid_request", "account is required");
  if (!Number.isSafeInteger(days) || days < 1 || days > 90) {
    return failure(400, "invalid_request", "days must be between 1 and 90");
  }
  return Response.json({ account, through, entries: statement(account, through, days) });
}

interface SyncOutcome {
  readonly connectionId: string;
  readonly accountId: string;
  readonly created: number;
  readonly updated: number;
  readonly outcome: string;
}

/**
 * Every connected account, in one pass. One connection's failure is recorded
 * on that connection and does not stop the rest: a scheduled job that gives
 * up on the first error leaves the other households unsynced with nothing
 * saying why.
 */
async function sync(request: Request): Promise<Response> {
  const through = todayFrom(request);
  const reason = "scheduled institution sync";
  const connections = await service(reason)
    .documents(CONNECTIONS)
    .query({
      predicates: [{ field: "kind", operator: "eq", value: "institution" }],
      sort: [],
      cursor: null,
      limit: 200,
    });
  const outcomes: SyncOutcome[] = [];
  for (const document of connections.documents) {
    if (document._deleted) continue;
    const connection = document.body;
    if (connection.status !== "connected") continue;
    const accountId = typeof connection.account_id === "string" ? connection.account_id : "";
    const externalId = typeof connection.external_id === "string" ? connection.external_id : "";
    const householdId = typeof connection.household_id === "string" ? connection.household_id : "";
    if (accountId === "" || externalId === "" || householdId === "") continue;
    try {
      const outcome = await syncConnection(document, {
        accountId,
        externalId,
        householdId,
        through,
      });
      outcomes.push(outcome);
    } catch (error) {
      const detail = error instanceof Error ? error.message.slice(0, 200) : "sync failed";
      await recordSync(document, `error: ${detail}`);
      outcomes.push({
        connectionId: String(connection.id),
        accountId,
        created: 0,
        updated: 0,
        outcome: `error: ${detail}`,
      });
    }
  }
  return Response.json({ ok: true, synced: outcomes.length, outcomes });
}

async function syncConnection(
  document: FunctionDocument,
  target: { accountId: string; externalId: string; householdId: string; through: string },
): Promise<SyncOutcome> {
  const reason = `institution sync for account ${target.accountId}`;
  const entries = statement(target.externalId, target.through, WINDOW_DAYS);
  const currency = typeof document.body.currency === "string" ? document.body.currency : "USD";
  let created = 0;
  let updated = 0;
  for (const entry of entries) {
    const id = transactionId(target.accountId, entry.externalId);
    const existing = await read(reason, TRANSACTIONS, id);
    const now = Date.now();
    const body: JsonObject = {
      id,
      household_id: target.householdId,
      created_at: (existing?.body.created_at as number | undefined) ?? now,
      updated_at: now,
      account_id: target.accountId,
      date: entry.date,
      amount: entry.amount,
      currency,
      description: entry.description,
      external_id: entry.externalId,
      tags: [],
      splits: [],
    };
    if (existing === null || existing._deleted) {
      await write(reason, TRANSACTIONS, id, body, null);
      created += 1;
    } else if (
      existing.body.amount !== entry.amount ||
      existing.body.date !== entry.date ||
      existing.body.description !== entry.description
    ) {
      // An entry that settled differently is the same transaction, corrected.
      await write(reason, TRANSACTIONS, id, body, existing.revision);
      updated += 1;
    }
  }
  const outcome = `imported ${created}, corrected ${updated}`;
  await recordSync(document, outcome);
  return {
    connectionId: String(document.body.id),
    accountId: target.accountId,
    created,
    updated,
    outcome,
  };
}

/** The connection's own record of when it last ran and what happened. */
async function recordSync(document: FunctionDocument, outcome: string): Promise<void> {
  const reason = "record an institution sync";
  const current = await read(reason, CONNECTIONS, String(document.body.id));
  if (current === null || current._deleted) return;
  await write(
    reason,
    CONNECTIONS,
    String(document.body.id),
    {
      ...current.body,
      updated_at: Date.now(),
      last_sync_at: Date.now(),
      last_sync_outcome: outcome.slice(0, 200),
    },
    current.revision,
  );
}

/**
 * `.` rather than `:`: a document id holding a character
 * `encodeURIComponent` escapes cannot be written from a function (findings
 * log #12), and this id is written from one on every sync.
 */
export function transactionId(accountId: string, externalId: string): string {
  return `txn_${accountId}.${externalId}`.replaceAll(/[^A-Za-z0-9_.-]/gu, "-").slice(0, 128);
}

function service(reason: string): ServiceFunctionClient {
  return createServiceClient({
    endpoint: environment("MAKO_API_URL"),
    projectId: environment("MAKO_PROJECT_ID"),
    environmentId: environment("MAKO_ENVIRONMENT_ID"),
    serviceCredential: serviceCredential(),
    reason,
    // A service request's quota reservation is keyed by its request id, and a
    // reused one is refused as a conflict (findings log #7a), so every call
    // gets its own.
    requestId: `req_${randomSuffix()}${randomSuffix()}`,
  });
}

async function read(
  reason: string,
  collectionId: string,
  documentId: string,
): Promise<FunctionDocument | null> {
  return service(reason).documents(collectionId).get(documentId);
}

async function write(
  reason: string,
  collectionId: string,
  documentId: string,
  body: JsonObject,
  expectedRevision: string | null,
): Promise<void> {
  await service(reason)
    .documents(collectionId)
    .mutate(documentId, {
      mutationId: `rational-sync-${randomSuffix()}${randomSuffix()}`,
      operation: expectedRevision === null ? "create" : "update",
      expectedRevision,
      schemaVersion: SCHEMA_VERSION,
      body,
    });
}

/** The scheduler may name the day, so a test can ask for a fixed window. */
function todayFrom(request: Request): string {
  const header = request.headers.get("x-rational-today");
  return header !== null && /^\d{4}-\d{2}-\d{2}$/u.test(header) ? header : today();
}

function today(): string {
  return new Date().toISOString().slice(0, 10);
}

function randomSuffix(): string {
  return crypto.randomUUID().replaceAll("-", "");
}

class RouteError extends Error {
  readonly status: number;
  readonly code: string;

  constructor(status: number, code: string, message: string) {
    super(message);
    this.status = status;
    this.code = code;
  }
}

function environment(name: string): string {
  const value = Deno.env.get(name);
  if (value === undefined || value === "") {
    throw new RouteError(500, "internal", `${name} is not configured for this function`);
  }
  return value;
}

function failure(status: number, code: string, message: string): Response {
  return Response.json({ error: { code, message } }, { status });
}
