/**
 * Rational's `institution-sync` function: the scheduled half of an account
 * that syncs itself.
 *
 * Routes (JSON in and out):
 *   POST /sync                  every connected account, one statement each
 *   GET  /institution/statement the simulated bank, called over HTTP like a
 *                               real one would be
 *   GET  /plaid/status          whether this deployment holds Plaid credentials
 *   POST /plaid/link-token      a Link token for the signed-in member
 *   POST /plaid/exchange        public token in, connection id out; the access
 *                               token lands only in `plaid_items`
 *
 * The schedule invokes `/sync` every fifteen minutes. What matters is not the
 * bank -- that one is simulated and deterministic -- but everything around
 * it: a schedule that fires with nobody signed in, a function writing with a
 * service credential, and transactions that must not double when a sync runs
 * twice over an overlapping window. Idempotence is by `(account,
 * external_id)`, which is also the transaction's document id, so a repeated
 * entry is an update of the same document rather than a second one.
 *
 * What a sync imports arrives unreviewed: the review queue is what a member
 * has not looked at yet, and nothing the institution sends has been looked
 * at. A connection whose pass fails is marked `error` and, when the household
 * asked, raises one `sync_error` alert that stays one alert however many
 * passes keep failing; the next pass retries it, and a pass that succeeds
 * marks it `connected` again.
 */
import {
  createFunctionClientFromRequest,
  createServiceClient,
  type FunctionDocument,
  type JsonObject,
  type ServiceFunctionClient,
} from "@mako-cloud/edge-sdk";
import {
  type AlertConnection,
  type AlertSettingLike,
  type AlertTransaction,
  type FiredAlert,
  firedAlerts,
} from "../shared/alerts.ts";
import {
  parseExchange,
  parseLinkToken,
  parseSyncPage,
  plaidConnectionBody,
  plaidConnectionId,
  plaidItemBody,
  removedIds,
  syncEntries,
} from "../shared/plaid.ts";
import { RUN_KEY_HEADER, runKeyMatches } from "../shared/run-key.ts";
import { serviceCredential } from "./credential.ts";
import { statement, transactionId } from "./institution.ts";

declare const Deno: { readonly env: { get(name: string): string | undefined } };

/** The schema version of `mako/collections.json`. */
const SCHEMA_VERSION = 3;
const CONNECTIONS = "connections";
const TRANSACTIONS = "transactions";
const ACCOUNTS = "accounts";
const ALERTS = "alerts";
/** The credential and cursor store no application user can open. */
const PLAID_ITEMS = "plaid_items";
/**
 * Plaid's sandbox origin -- the one external host this deployment declares.
 * Hardcoded rather than configurable: the egress allowlist names it, and a
 * URL a function could be talked into changing would hollow that grant out.
 */
const PLAID_HOST = "https://sandbox.plaid.com";
/** Pages one connection may pull in one run; the cursor carries the rest. */
const PLAID_PAGE_LIMIT = 10;
/** How far back each sync asks. Overlap is deliberate: it is what proves the
 * idempotence, and it is what a real institution needs anyway because entries
 * settle days after they happen. */
const WINDOW_DAYS = 10;

export default {
  async fetch(request: Request): Promise<Response> {
    const url = new URL(request.url);
    const segments = url.pathname.split("/").filter((part) => part !== "");
    const route = segments[segments.length - 1] ?? "";
    const group = segments[segments.length - 2] ?? "";
    try {
      if (route === "statement") return institutionRoute(url);
      if (group === "plaid") {
        if (route === "status" && request.method === "GET") return await plaidStatus(request);
        if (request.method !== "POST") {
          return failure(405, "invalid_request", "plaid routes are POST");
        }
        if (route === "link-token") return await plaidLinkToken(request);
        if (route === "exchange") return await plaidExchange(request);
        return failure(404, "not_found", `unknown plaid route ${route}`);
      }
      if (request.method !== "POST") {
        return failure(405, "invalid_request", "institution-sync routes are POST");
      }
      if (route === "sync") {
        authorizeRun(request);
        return await sync(request);
      }
      return failure(404, "not_found", `unknown institution-sync route ${route}`);
    } catch (error) {
      if (error instanceof RouteError) return failure(error.status, error.code, error.message);
      // The caller is told nothing but "it failed"; the operator is told
      // what -- including the message, which JSON serialization of an Error
      // drops (its own properties are not enumerable).
      console.error(error instanceof Error ? `${error.name}: ${error.message}` : error);
      return failure(500, "internal", "the institution-sync function failed");
    }
  },
};

// --- Plaid: a real aggregator through the declared egress host -------------

interface PlaidCredentials {
  readonly clientId: string;
  readonly secret: string;
}

/**
 * The Plaid credentials this deployment holds, or null when it holds none.
 * Secrets are attached per deployment, and reading an unattached name is
 * refused by the sandbox rather than answered with undefined -- so absence is
 * caught, and means the Plaid routes honestly report themselves unconfigured
 * instead of failing one fetch deep.
 */
function plaidCredentials(): PlaidCredentials | null {
  try {
    const clientId = Deno.env.get("PLAID_CLIENT_ID") ?? "";
    const secret = Deno.env.get("PLAID_SECRET") ?? "";
    return clientId !== "" && secret !== "" ? { clientId, secret } : null;
  } catch {
    return null;
  }
}

interface Member {
  readonly userId: string;
  /** Household id → role, from the claim the runtime verified. */
  readonly households: Record<string, string>;
}

/** The verified caller, or a 401 that says what to do about it. */
async function memberOf(request: Request): Promise<Member> {
  const client = createFunctionClientFromRequest({
    request,
    endpoint: environment("MAKO_API_URL"),
    projectId: environment("MAKO_PROJECT_ID"),
    environmentId: environment("MAKO_ENVIRONMENT_ID"),
  });
  let user: { id: string };
  try {
    user = await client.auth.getUser();
  } catch {
    throw new RouteError(401, "unauthenticated", "sign in to connect an institution");
  }
  return { userId: user.id, households: claimedHouseholds(request) };
}

function claimedHouseholds(request: Request): Record<string, string> {
  const token = request.headers.get("x-mako-caller-authorization");
  const payload = token?.split(".")[1];
  if (payload === undefined) return {};
  try {
    const padded = payload.replaceAll("-", "+").replaceAll("_", "/");
    const binary = atob(padded.padEnd(padded.length + ((4 - (padded.length % 4)) % 4), "="));
    const claims = JSON.parse(binary) as { trusted_claims?: { households?: unknown } };
    const value = claims.trusted_claims?.households;
    if (typeof value !== "object" || value === null || Array.isArray(value)) return {};
    const map: Record<string, string> = {};
    for (const [householdId, role] of Object.entries(value as Record<string, unknown>)) {
      if (typeof role === "string") map[householdId] = role;
    }
    return map;
  } catch {
    return {};
  }
}

function requireWriter(member: Member, householdId: string): void {
  const role = member.households[householdId];
  if (role !== "owner" && role !== "editor") {
    throw new RouteError(403, "forbidden", "only an owner or editor may connect an institution");
  }
}

/**
 * One call to Plaid. The caller learns that the institution refused, and the
 * operator's log learns Plaid's own error code; the response body -- which
 * carries request ids and echoes of what was sent -- goes no further.
 */
async function plaidCall(
  credentials: PlaidCredentials,
  path: string,
  payload: JsonObject,
): Promise<unknown> {
  const response = await fetch(`${PLAID_HOST}${path}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      client_id: credentials.clientId,
      secret: credentials.secret,
      ...payload,
    }),
  });
  const body: unknown = await response.json().catch(() => null);
  if (!response.ok) {
    const code =
      typeof body === "object" && body !== null && "error_code" in body
        ? String((body as { error_code: unknown }).error_code).slice(0, 64)
        : "unknown";
    console.error(`plaid ${path} refused: status ${response.status} code ${code}`);
    throw new RouteError(502, "institution_error", `the institution refused ${path}`);
  }
  return body;
}

/** Whether this deployment can talk to Plaid at all; the UI asks before offering. */
async function plaidStatus(request: Request): Promise<Response> {
  await memberOf(request);
  return Response.json({ configured: plaidCredentials() !== null });
}

async function plaidLinkToken(request: Request): Promise<Response> {
  const member = await memberOf(request);
  const credentials = plaidCredentials();
  if (credentials === null) {
    return failure(503, "not_configured", "Plaid is not configured for this deployment");
  }
  const answer = await plaidCall(credentials, "/link/token/create", {
    client_name: "Rational",
    language: "en",
    country_codes: ["US"],
    user: { client_user_id: member.userId },
    products: ["transactions"],
  });
  return Response.json({ linkToken: parseLinkToken(answer) });
}

/**
 * The handover: the browser's short-lived public token becomes an access
 * token that never reaches a browser again. The token is written only to
 * `plaid_items` -- the collection whose policy allows no application user
 * anything -- and the response carries the connection id and nothing else.
 */
async function plaidExchange(request: Request): Promise<Response> {
  const member = await memberOf(request);
  const credentials = plaidCredentials();
  if (credentials === null) {
    return failure(503, "not_configured", "Plaid is not configured for this deployment");
  }
  let body: JsonObject;
  try {
    body = (await request.json()) as JsonObject;
  } catch {
    throw new RouteError(400, "invalid_request", "a JSON body is required");
  }
  const publicToken = typeof body.publicToken === "string" ? body.publicToken : "";
  const householdId = typeof body.householdId === "string" ? body.householdId : "";
  const accountId = typeof body.accountId === "string" ? body.accountId : "";
  const institution =
    typeof body.institution === "string" && body.institution !== ""
      ? body.institution.slice(0, 200)
      : "Plaid";
  if (publicToken === "" || householdId === "" || accountId === "") {
    throw new RouteError(
      400,
      "invalid_request",
      "publicToken, householdId, and accountId are required",
    );
  }
  requireWriter(member, householdId);
  // The account must be the household's own: without this, an editor of one
  // household could pour an institution's transactions into another's books.
  const reason = `plaid link for household ${householdId}`;
  const account = await read(reason, ACCOUNTS, accountId);
  if (account === null || account._deleted || account.body.household_id !== householdId) {
    throw new RouteError(403, "forbidden", "the account does not belong to that household");
  }

  const exchanged = parseExchange(
    await plaidCall(credentials, "/item/public_token/exchange", { public_token: publicToken }),
  );
  const connectionId = plaidConnectionId(exchanged.itemId);
  const now = Date.now();

  // The item first: a connection without its item is a link that cannot sync,
  // but an item without its connection is only an orphan a relink absorbs.
  const existingItem = await read(reason, PLAID_ITEMS, connectionId);
  if (existingItem === null || existingItem._deleted) {
    await write(
      reason,
      PLAID_ITEMS,
      connectionId,
      plaidItemBody({
        connectionId,
        householdId,
        itemId: exchanged.itemId,
        accessToken: exchanged.accessToken,
        cursor: "",
        now,
      }),
      null,
    );
  } else {
    // A relink of the same item keeps its cursor: history already imported
    // does not need importing again.
    await write(
      reason,
      PLAID_ITEMS,
      connectionId,
      { ...existingItem.body, access_token: exchanged.accessToken, updated_at: now },
      existingItem.revision,
    );
  }

  const existingConnection = await read(reason, CONNECTIONS, connectionId);
  if (existingConnection === null || existingConnection._deleted) {
    await write(
      reason,
      CONNECTIONS,
      connectionId,
      plaidConnectionBody({
        connectionId,
        householdId,
        accountId,
        itemId: exchanged.itemId,
        institutionName: institution,
        now,
      }),
      null,
    );
  } else {
    await write(
      reason,
      CONNECTIONS,
      connectionId,
      {
        ...existingConnection.body,
        status: "connected",
        account_id: accountId,
        institution,
        updated_at: now,
      },
      existingConnection.revision,
    );
  }
  return Response.json({ connectionId });
}

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
  readonly alerted: number;
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
    if (!syncable(connection)) continue;
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
      outcomes.push(await recordFailure(document, { accountId, householdId, through }, error));
    }
  }
  outcomes.push(...(await syncPlaidPass(reason, through)));
  return Response.json({ ok: true, synced: outcomes.length, outcomes });
}

/**
 * Whether a pass should try the connection. A connection in `error` is tried
 * again -- that is how it gets back to `connected`, and how a household finds
 * out the institution is working again without doing anything. A member's
 * `disconnected` is not: they said stop.
 */
function syncable(connection: JsonObject): boolean {
  return connection.status === "connected" || connection.status === "error";
}

/**
 * One connection's failed pass: recorded on the connection, reported as an
 * alert when the household asked for one, and returned as an outcome so the
 * run's response says which connection and why. The alert is best effort --
 * a failure to write it is logged, and must not turn a recorded failure into
 * a pass that looks like it never happened.
 */
async function recordFailure(
  document: FunctionDocument,
  target: { accountId: string; householdId: string; through: string },
  error: unknown,
): Promise<SyncOutcome> {
  const detail = error instanceof Error ? error.message.slice(0, 200) : "sync failed";
  const outcome = `error: ${detail}`;
  await recordSync(document, outcome, "error");
  let alerted = 0;
  try {
    alerted = await raiseAlerts(target.householdId, target.through, {
      arrived: [],
      failing: {
        id: String(document.body.id),
        institution:
          typeof document.body.institution === "string" && document.body.institution !== ""
            ? document.body.institution
            : "The institution",
        failing: true,
        outcome,
      },
      currency: await accountCurrency(target.accountId),
    });
  } catch (alertError) {
    console.error(
      alertError instanceof Error
        ? `sync_error alert not written: ${alertError.message}`
        : "sync_error alert not written",
    );
  }
  return {
    connectionId: String(document.body.id),
    accountId: target.accountId,
    created: 0,
    updated: 0,
    alerted,
    outcome,
  };
}

/**
 * The currency to stamp on an alert about nothing priced. The household's own
 * would be the natural one, but this credential is scoped to what a sync
 * touches and `households` is not in it; the connection's account is, and
 * its currency is the one the connection's transactions carry. When even that
 * cannot be read -- the pass may have failed because nothing can be -- the
 * engine falls back to ISO 4217's code for "no currency".
 */
async function accountCurrency(accountId: string): Promise<string | undefined> {
  try {
    const account = await read("currency for a sync alert", ACCOUNTS, accountId);
    return account !== null && typeof account.body.currency === "string"
      ? account.body.currency
      : undefined;
  } catch {
    return undefined;
  }
}

/**
 * The same pass, for connections whose institution is real. A deployment
 * without Plaid credentials skips them without recording an error on each:
 * the connection is not broken, the deployment is undressed for it, and a
 * record that said "error" every fifteen minutes would bury the one that
 * matters.
 */
async function syncPlaidPass(reason: string, through: string): Promise<SyncOutcome[]> {
  const connections = await service(reason)
    .documents(CONNECTIONS)
    .query({
      predicates: [{ field: "kind", operator: "eq", value: "plaid" }],
      sort: [],
      cursor: null,
      limit: 200,
    });
  const outcomes: SyncOutcome[] = [];
  const credentials = plaidCredentials();
  for (const document of connections.documents) {
    if (document._deleted) continue;
    const connection = document.body;
    if (!syncable(connection)) continue;
    const accountId = typeof connection.account_id === "string" ? connection.account_id : "";
    const householdId = typeof connection.household_id === "string" ? connection.household_id : "";
    if (accountId === "" || householdId === "") continue;
    if (credentials === null) {
      outcomes.push({
        connectionId: String(connection.id),
        accountId,
        created: 0,
        updated: 0,
        alerted: 0,
        outcome: "skipped: plaid is not configured",
      });
      continue;
    }
    try {
      outcomes.push(
        await syncPlaidConnection(document, credentials, { accountId, householdId, through }),
      );
    } catch (error) {
      outcomes.push(await recordFailure(document, { accountId, householdId, through }, error));
    }
  }
  return outcomes;
}

/**
 * One Plaid connection, brought current under its cursor. Idempotence is the
 * same one the simulator proved -- the transaction's id is derived from
 * `(account, external_id)` -- and the cursor advances only after the page's
 * writes committed, so a crashed pass replays a page into upserts that change
 * nothing. An entry the institution withdrew (a pending charge that posted)
 * is deleted, not annotated: the institution says it no longer exists, and
 * its replacement arrives in the same page.
 */
async function syncPlaidConnection(
  document: FunctionDocument,
  credentials: PlaidCredentials,
  target: { accountId: string; householdId: string; through: string },
): Promise<SyncOutcome> {
  const reason = `plaid sync for account ${target.accountId}`;
  const connectionId = String(document.body.id);
  const item = await read(reason, PLAID_ITEMS, connectionId);
  if (item === null || item._deleted) {
    throw new Error("the connection's credentials are gone; relink the institution");
  }
  const accessToken = String(item.body.access_token ?? "");
  const account = await read(reason, ACCOUNTS, target.accountId);
  const currency =
    account !== null && typeof account.body.currency === "string" ? account.body.currency : "USD";

  let cursor = typeof item.body.cursor === "string" ? item.body.cursor : "";
  let created = 0;
  let updated = 0;
  let removed = 0;
  const arrived: AlertTransaction[] = [];
  for (let pages = 0; pages < PLAID_PAGE_LIMIT; pages += 1) {
    const page = parseSyncPage(
      await plaidCall(credentials, "/transactions/sync", {
        access_token: accessToken,
        count: 100,
        ...(cursor === "" ? {} : { cursor }),
      }),
    );
    for (const entry of syncEntries(page, currency)) {
      const id = transactionId(target.accountId, entry.externalId);
      const existing = await read(reason, TRANSACTIONS, id);
      const now = Date.now();
      // No `reviewed`: what the institution sends has not been looked at, and
      // the review queue is exactly the imported transactions without the
      // flag. Setting it false would say the same thing in a way the queue
      // does not read.
      const body: JsonObject = {
        id,
        household_id: target.householdId,
        created_at: (existing?.body.created_at as number | undefined) ?? now,
        updated_at: now,
        account_id: target.accountId,
        date: entry.date,
        amount: entry.amount,
        currency: entry.currency,
        description: entry.description,
        external_id: entry.externalId,
        tags: [],
        splits: [],
      };
      const arrival: AlertTransaction = {
        id,
        account_id: target.accountId,
        date: entry.date,
        amount: entry.amount,
        currency: entry.currency,
        description: entry.description,
      };
      if (existing === null || existing._deleted) {
        await write(reason, TRANSACTIONS, id, body, null);
        arrived.push(arrival);
        created += 1;
      } else if (
        existing.body.amount !== entry.amount ||
        existing.body.date !== entry.date ||
        existing.body.description !== entry.description
      ) {
        await write(reason, TRANSACTIONS, id, body, existing.revision);
        arrived.push(arrival);
        updated += 1;
      }
    }
    for (const withdrawnId of removedIds(page)) {
      const id = transactionId(target.accountId, withdrawnId);
      const existing = await read(reason, TRANSACTIONS, id);
      if (existing !== null && !existing._deleted) {
        await removeDocument(reason, TRANSACTIONS, id, existing);
        removed += 1;
      }
    }
    // Only now, with the page's writes down, does the cursor move.
    cursor = page.next_cursor;
    const current = await read(reason, PLAID_ITEMS, connectionId);
    if (current !== null && !current._deleted) {
      await write(
        reason,
        PLAID_ITEMS,
        connectionId,
        { ...current.body, cursor, updated_at: Date.now() },
        current.revision,
      );
    }
    if (!page.has_more) break;
  }

  const alerted = await raiseAlerts(target.householdId, target.through, { arrived });
  const outcome = `imported ${created}, corrected ${updated}, removed ${removed}`;
  await recordSync(document, outcome, "connected");
  return {
    connectionId,
    accountId: target.accountId,
    created,
    updated,
    alerted,
    outcome,
  };
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
  const arrived: AlertTransaction[] = [];
  for (const entry of entries) {
    const id = transactionId(target.accountId, entry.externalId);
    const existing = await read(reason, TRANSACTIONS, id);
    const now = Date.now();
    // No `reviewed`, for the same reason as the Plaid pass: an import is what
    // the review queue is for.
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
    const arrival: AlertTransaction = {
      id,
      account_id: target.accountId,
      date: entry.date,
      amount: entry.amount,
      currency,
      description: entry.description,
    };
    if (existing === null || existing._deleted) {
      await write(reason, TRANSACTIONS, id, body, null);
      arrived.push(arrival);
      created += 1;
    } else if (
      existing.body.amount !== entry.amount ||
      existing.body.date !== entry.date ||
      existing.body.description !== entry.description
    ) {
      // An entry that settled differently is the same transaction, corrected.
      await write(reason, TRANSACTIONS, id, body, existing.revision);
      arrived.push(arrival);
      updated += 1;
    }
  }
  const alerted = await raiseAlerts(target.householdId, target.through, { arrived });
  const outcome = `imported ${created}, corrected ${updated}`;
  await recordSync(document, outcome, "connected");
  return {
    connectionId: String(document.body.id),
    accountId: target.accountId,
    created,
    updated,
    alerted,
    outcome,
  };
}

/** What one pass gives the alert engine: what it wrote, and whether it failed. */
interface SyncAlertSubject {
  readonly arrived: readonly AlertTransaction[];
  /** The connection whose pass just failed, when one did. */
  readonly failing?: AlertConnection;
  /** Stamped on an alert about nothing priced; the engine has a fallback. */
  readonly currency?: string | undefined;
}

/**
 * A large charge should not have to wait until two in the morning, and a
 * connection that has stopped syncing should not wait at all.
 *
 * So the sync evaluates the household's settings over what this pass knows,
 * and nothing else: the transactions it just wrote and the connection it just
 * failed. Every setting is handed to the engine, with the household's
 * accounts, budgets, bills, and goals empty -- whether a budget is over, an
 * account low, a bill due, or a goal reached is a question about the
 * household as a whole, and `nightly`, which has read the whole household,
 * answers those. Over an empty subject those settings fire nothing here. The
 * alert ids are derived the same way in both, so whichever runs first fires
 * the alert and the other finds it already there -- which is also what makes
 * a connection that keeps failing one alert rather than one per quarter hour.
 */
async function raiseAlerts(
  householdId: string,
  day: string,
  subject: SyncAlertSubject,
): Promise<number> {
  if (subject.arrived.length === 0 && subject.failing === undefined) return 0;
  const reason = `institution sync alerts for ${householdId}`;
  const stored = await service(reason)
    .documents(ALERTS)
    .query({
      predicates: [
        { field: "household_id", operator: "eq", value: householdId },
        { field: "kind", operator: "eq", value: "setting" },
      ],
      sort: [],
      cursor: null,
      limit: 200,
    });
  const settings = stored.documents
    .filter((entry) => !entry._deleted)
    .map((entry) => entry.body as unknown as AlertSettingLike);
  if (settings.length === 0) return 0;
  const fired = firedAlerts(settings, {
    householdId,
    day,
    transactions: subject.arrived,
    accounts: [],
    budgets: [],
    bills: [],
    goals: [],
    connections: subject.failing === undefined ? [] : [subject.failing],
    ...(subject.currency === undefined ? {} : { currency: subject.currency }),
    existingIds: new Set(),
  });
  let written = 0;
  for (const alert of fired) {
    if (await writeAlert(reason, householdId, alert)) written += 1;
  }
  return written;
}

async function writeAlert(
  reason: string,
  householdId: string,
  alert: FiredAlert,
): Promise<boolean> {
  const existing = await read(reason, ALERTS, alert.id);
  if (existing !== null && !existing._deleted) return false;
  const now = Date.now();
  await write(
    reason,
    ALERTS,
    alert.id,
    {
      id: alert.id,
      household_id: householdId,
      created_at: now,
      updated_at: now,
      kind: "alert",
      alert_kind: alert.alert_kind,
      fired_at: now,
      message: alert.message,
      amount: alert.amount,
      currency: alert.currency,
      read: false,
      ...(alert.transaction_id === undefined ? {} : { transaction_id: alert.transaction_id }),
      ...(alert.account_id === undefined ? {} : { account_id: alert.account_id }),
      ...(alert.connection_id === undefined ? {} : { connection_id: alert.connection_id }),
    },
    null,
  );
  return true;
}

/**
 * The connection's own record of when it last ran, what happened, and where
 * that leaves it: `error` after a failed pass, `connected` again after one
 * that worked. A member who disconnected the institution while this pass ran
 * has the last word -- the sync reports on a connection, it does not reopen
 * one -- so a `disconnected` found on the fresh read is kept.
 */
async function recordSync(
  document: FunctionDocument,
  outcome: string,
  status: "connected" | "error",
): Promise<void> {
  const reason = "record an institution sync";
  const current = await read(reason, CONNECTIONS, String(document.body.id));
  if (current === null || current._deleted) return;
  await write(
    reason,
    CONNECTIONS,
    String(document.body.id),
    {
      ...current.body,
      ...(current.body.status === "disconnected" ? {} : { status }),
      updated_at: Date.now(),
      last_sync_at: Date.now(),
      last_sync_outcome: outcome.slice(0, 200),
    },
    current.revision,
  );
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

/** A delete names the revision it read, like every other write. */
async function removeDocument(
  reason: string,
  collectionId: string,
  documentId: string,
  existing: FunctionDocument,
): Promise<void> {
  await service(reason)
    .documents(collectionId)
    .mutate(documentId, {
      mutationId: `rational-sync-${randomSuffix()}${randomSuffix()}`,
      operation: "delete",
      expectedRevision: existing.revision,
      schemaVersion: SCHEMA_VERSION,
      body: existing.body,
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

/**
 * The run key this deployment was given, or "" when it has none -- in which
 * case nothing may run it, because a function that falls open when its secret
 * is missing has no gate at all.
 */
function runKey(): string {
  return Deno.env.get("RATIONAL_RUN_KEY") ?? "";
}

function authorizeRun(request: Request): void {
  if (!runKeyMatches(request.headers.get(RUN_KEY_HEADER), runKey())) {
    throw new RouteError(401, "unauthenticated", "this function runs on a schedule");
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
