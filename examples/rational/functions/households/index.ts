/**
 * Rational's `households` function: the only writer of membership.
 *
 * Membership is a claim on the member's token — `households: {"<id>": role}` —
 * because a document policy can read a trusted claim but cannot join a
 * membership table. Only trusted code may write a claim, so every change goes
 * through here: the caller is the identity the runtime verified, the caller's
 * own claim decides whether they may make the change, and the claim and the
 * readable `memberships` projection are written together with a service
 * credential under a reason that names the action.
 *
 * Routes (all POST, JSON in and out):
 *   /create {name, currency}                  → {householdId, role}
 *   /invite {householdId, email, role}        → {invited, role}    (owner only)
 *   /accept {householdId}                     → {householdId, role}
 *   /role   {householdId, userId, role}       → {householdId, userId, role} (owner)
 *   /remove {householdId, userId}             → {householdId, userId} (owner)
 *
 * The caller's token carries the claim it changes, so the app refreshes its
 * session after every call; the write advances the member's authorization
 * epoch, which is what makes a removal take effect on their device at once.
 */
import {
  createFunctionClientFromRequest,
  createServiceClient,
  type FunctionDocument,
  type JsonObject,
  type ServiceFunctionClient,
} from "@mako-cloud/edge-sdk";

import { serviceCredential } from "./credential.ts";

declare const Deno: { readonly env: { get(name: string): string | undefined } };

/** The schema version of `mako/collections.json`. */
const SCHEMA_VERSION = 1;
const ROLES = ["owner", "editor", "viewer"] as const;
type Role = (typeof ROLES)[number];

const HOUSEHOLDS = "households";
const MEMBERSHIPS = "memberships";

export default {
  async fetch(request: Request): Promise<Response> {
    if (request.method !== "POST") {
      return failure(405, "invalid_request", "households routes are POST");
    }
    const segments = new URL(request.url).pathname.split("/").filter((part) => part !== "");
    const route = segments[segments.length - 1] ?? "";
    let body: JsonObject;
    try {
      body = (await request.json()) as JsonObject;
    } catch {
      return failure(400, "invalid_request", "a JSON body is required");
    }
    if (typeof body !== "object" || body === null || Array.isArray(body)) {
      return failure(400, "invalid_request", "a JSON object body is required");
    }
    try {
      const caller = await callerOf(request);
      switch (route) {
        case "create":
          return await create(caller, body);
        case "invite":
          return await invite(caller, body);
        case "accept":
          return await accept(caller, body);
        case "role":
          return await setRole(caller, body);
        case "remove":
          return await removeMember(caller, body);
        default:
          return failure(404, "not_found", `unknown households route ${route}`);
      }
    } catch (error) {
      if (error instanceof RouteError) {
        return failure(error.status, error.code, error.message);
      }
      return failure(500, "internal", "the households function failed");
    }
  },
};

// --- caller --------------------------------------------------------------

interface Caller {
  readonly userId: string;
  readonly email: string;
  /** The households claim the verified token carries, by household id. */
  readonly households: Record<string, Role>;
}

/**
 * Who is calling. The identity comes from the data plane (the SDK forwards
 * the verified token); the roles come from the same token's trusted claims,
 * which the runtime verified before this function ran and which is exactly
 * what the document policies will read on the writes that follow.
 */
async function callerOf(request: Request): Promise<Caller> {
  const client = createFunctionClientFromRequest({
    request,
    endpoint: environment("MAKO_API_URL"),
    projectId: environment("MAKO_PROJECT_ID"),
    environmentId: environment("MAKO_ENVIRONMENT_ID"),
  });
  let user: { id: string; email: string };
  try {
    user = await client.auth.getUser();
  } catch {
    throw new RouteError(401, "unauthenticated", "sign in to change a household");
  }
  return {
    userId: user.id,
    email: user.email.trim().toLowerCase(),
    households: claimedHouseholds(request),
  };
}

function claimedHouseholds(request: Request): Record<string, Role> {
  const token = request.headers.get("x-mako-caller-authorization");
  if (token === null) return {};
  const payload = token.split(".")[1];
  if (payload === undefined) return {};
  try {
    const json = new TextDecoder().decode(base64UrlBytes(payload));
    const claims = JSON.parse(json) as { trusted_claims?: { households?: unknown } };
    return roleMap(claims.trusted_claims?.households);
  } catch {
    return {};
  }
}

function base64UrlBytes(value: string): Uint8Array {
  const padded = value.replaceAll("-", "+").replaceAll("_", "/");
  const binary = atob(padded.padEnd(padded.length + ((4 - (padded.length % 4)) % 4), "="));
  return Uint8Array.from(binary, (character) => character.codePointAt(0) ?? 0);
}

function roleMap(value: unknown): Record<string, Role> {
  const map: Record<string, Role> = {};
  if (typeof value !== "object" || value === null || Array.isArray(value)) return map;
  for (const [householdId, role] of Object.entries(value as Record<string, unknown>)) {
    if (isRole(role)) map[householdId] = role;
  }
  return map;
}

// --- routes --------------------------------------------------------------

async function create(caller: Caller, body: JsonObject): Promise<Response> {
  const name = text(body.name, "name", 200);
  const currency = /^[A-Z]{3}$/u.test(String(body.currency ?? "")) ? String(body.currency) : "USD";
  const householdId = `hh_${randomSuffix()}`;
  const at = Date.now();
  const reason = `create household ${householdId} for ${caller.userId}`;
  await put(reason, HOUSEHOLDS, householdId, {
    id: householdId,
    household_id: householdId,
    created_at: at,
    updated_at: at,
    name,
    currency,
    owner_id: caller.userId,
  });
  await putMembership(reason, householdId, caller.userId, caller.email, "owner");
  await changeHousehold(reason, caller.userId, householdId, "owner");
  return success({ householdId, role: "owner" });
}

async function invite(caller: Caller, body: JsonObject): Promise<Response> {
  const householdId = text(body.householdId, "householdId", 128);
  requireOwner(caller, householdId);
  const email = text(body.email, "email", 320).toLowerCase();
  if (!email.includes("@")) throw new RouteError(400, "invalid_request", "email is not an address");
  const role = requireRole(body.role);
  const at = Date.now();
  const reason = `invite ${email} to household ${householdId} as ${role}`;
  const id = invitationId(householdId, await emailDigest(email));
  // The invitee may not have an account yet, so the invitation is a
  // membership document keyed by the address rather than by a user, and it
  // carries no claim: accepting is what creates one.
  await put(reason, MEMBERSHIPS, id, {
    id,
    household_id: householdId,
    created_at: at,
    updated_at: at,
    // A pending invitation names an address, not a user; the field is
    // required by the schema and indexed, so it is empty rather than absent.
    user_id: "",
    email,
    role,
    status: "invited",
    invited_by: caller.userId,
  });
  return success({ invited: email, role });
}

async function accept(caller: Caller, body: JsonObject): Promise<Response> {
  const householdId = text(body.householdId, "householdId", 128);
  const id = invitationId(householdId, await emailDigest(caller.email));
  const reason = `accept invitation to household ${householdId} as ${caller.userId}`;
  const invitation = await read(reason, MEMBERSHIPS, id);
  if (invitation === null || invitation.body.status !== "invited") {
    throw new RouteError(404, "not_found", "there is no invitation for this address");
  }
  const role = isRole(invitation.body.role) ? invitation.body.role : "viewer";
  await putMembership(reason, householdId, caller.userId, caller.email, role);
  await changeHousehold(reason, caller.userId, householdId, role);
  // The invitation is marked rather than deleted: the credential this
  // function holds may create, read, and update, and a projection that keeps
  // its history is the better record anyway.
  await put(reason, MEMBERSHIPS, String(invitation.body.id), {
    ...invitation.body,
    status: "accepted",
    user_id: caller.userId,
    updated_at: Date.now(),
  });
  return success({ householdId, role });
}

async function setRole(caller: Caller, body: JsonObject): Promise<Response> {
  const householdId = text(body.householdId, "householdId", 128);
  requireOwner(caller, householdId);
  const userId = text(body.userId, "userId", 128);
  const role = requireRole(body.role);
  const reason = `set the role of ${userId} in household ${householdId} to ${role}`;
  const membership = await read(reason, MEMBERSHIPS, membershipId(householdId, userId));
  if (membership === null || membership._deleted || membership.body.status !== "active") {
    throw new RouteError(404, "not_found", "that person is not a member of this household");
  }
  const email = typeof membership.body.email === "string" ? membership.body.email : "";
  await putMembership(reason, householdId, userId, email, role);
  await changeHousehold(reason, userId, householdId, role);
  return success({ householdId, userId, role });
}

async function removeMember(caller: Caller, body: JsonObject): Promise<Response> {
  const householdId = text(body.householdId, "householdId", 128);
  requireOwner(caller, householdId);
  const userId = text(body.userId, "userId", 128);
  const reason = `remove ${userId} from household ${householdId}`;
  const household = await read(reason, HOUSEHOLDS, householdId);
  if (household !== null && household.body.owner_id === userId) {
    throw new RouteError(400, "invalid_request", "the household's owner cannot be removed");
  }
  const id = membershipId(householdId, userId);
  const existing = await read(reason, MEMBERSHIPS, id);
  if (existing === null || existing._deleted || existing.body.status !== "active") {
    throw new RouteError(404, "not_found", "that person is not a member of this household");
  }
  // The claim goes first: it is what the policies read, so from here the
  // person is refused by the data plane whatever else happens below, and
  // their next token — the app refreshes on the epoch change — no longer
  // carries the household.
  await changeHousehold(reason, userId, householdId, null);
  await put(reason, MEMBERSHIPS, id, {
    ...existing.body,
    status: "removed",
    updated_at: Date.now(),
  });
  return success({ householdId, userId });
}

// --- service writes ------------------------------------------------------

/**
 * A service client per call, because the data plane keys a service request's
 * quota reservation by its request id and answers a reused one with
 * `503 unavailable` (findings log #7a). Each client also carries the reason
 * this particular change is audited under.
 */
function service(reason: string): ServiceFunctionClient {
  return createServiceClient({
    endpoint: environment("MAKO_API_URL"),
    projectId: environment("MAKO_PROJECT_ID"),
    environmentId: environment("MAKO_ENVIRONMENT_ID"),
    serviceCredential: serviceCredential(),
    reason,
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

async function put(
  reason: string,
  collectionId: string,
  documentId: string,
  body: JsonObject,
): Promise<void> {
  const existing = await read(reason, collectionId, documentId);
  const created = existing === null || existing._deleted;
  await service(reason)
    .documents(collectionId)
    .mutate(documentId, {
      mutationId: `rational-households-${randomSuffix()}${randomSuffix()}`,
      operation: created ? "create" : "update",
      expectedRevision: created ? null : existing.revision,
      schemaVersion: SCHEMA_VERSION,
      body: created ? body : { ...body, created_at: existing.body.created_at ?? body.created_at },
    });
}

async function putMembership(
  reason: string,
  householdId: string,
  userId: string,
  email: string,
  role: Role,
): Promise<void> {
  const at = Date.now();
  await put(reason, MEMBERSHIPS, membershipId(householdId, userId), {
    id: membershipId(householdId, userId),
    household_id: householdId,
    created_at: at,
    updated_at: at,
    user_id: userId,
    ...(email === "" ? {} : { email }),
    role,
    status: "active",
  });
}

/**
 * Change one household in a person's claim, leaving the rest of it — and the
 * rest of their app metadata — alone.
 *
 * `setAppMetadata` replaces a key whole, so the next value is composed out of
 * the current one, read through the service route. The epoch that read
 * returns goes back with the write: two invitations accepted at once would
 * otherwise each write the map they read and the later one would drop the
 * other's household. A conflict means somebody else changed this person's
 * claim first, so the whole read-compose-write is retried against what they
 * left.
 */
async function changeHousehold(
  reason: string,
  userId: string,
  householdId: string,
  role: Role | null,
): Promise<void> {
  const users = service(reason).users;
  for (let attempt = 0; attempt < 4; attempt += 1) {
    const current = await users.getAppMetadata(userId, reason);
    const households: Record<string, Role> = {};
    const held = current.appMetadata.households;
    if (typeof held === "object" && held !== null && !Array.isArray(held)) {
      for (const [id, value] of Object.entries(held)) {
        if (isRole(value)) households[id] = value;
      }
    }
    if (role === null) delete households[householdId];
    else households[householdId] = role;
    try {
      await users.setAppMetadata(
        userId,
        { households },
        { reason, expectedAuthorizationEpoch: current.authorizationEpoch },
      );
      return;
    } catch (error) {
      if ((error as { code?: string }).code !== "conflict") throw error;
    }
  }
  throw new RouteError(409, "conflict", "this member's access is being changed; try again");
}

// --- identifiers and helpers --------------------------------------------

/**
 * `.` rather than `:` on purpose: the service document route compares the raw
 * path segment with the body's id, and the SDK percent-encodes the path, so
 * an id holding a character `encodeURIComponent` escapes cannot be written
 * from a function at all (findings log #7c).
 */
function membershipId(householdId: string, userId: string): string {
  return `${householdId}.${userId}`;
}

function invitationId(householdId: string, digest: string): string {
  return `${householdId}.invite.${digest}`;
}

async function emailDigest(email: string): Promise<string> {
  const bytes = new TextEncoder().encode(email.trim().toLowerCase());
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  return Array.from(new Uint8Array(digest).slice(0, 16))
    .map((byte) => byte.toString(16).padStart(2, "0"))
    .join("");
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

function requireOwner(caller: Caller, householdId: string): void {
  if (caller.households[householdId] !== "owner") {
    throw new RouteError(403, "permission_denied", "only the household's owner may do that");
  }
}

function requireRole(value: unknown): Role {
  if (!isRole(value)) {
    throw new RouteError(400, "invalid_request", "role must be owner, editor, or viewer");
  }
  return value;
}

function isRole(value: unknown): value is Role {
  return value === "owner" || value === "editor" || value === "viewer";
}

function text(value: unknown, field: string, maximum: number): string {
  const trimmed = typeof value === "string" ? value.trim() : "";
  if (trimmed === "" || trimmed.length > maximum) {
    throw new RouteError(400, "invalid_request", `${field} is required`);
  }
  return trimmed;
}

function environment(name: string): string {
  const value = Deno.env.get(name);
  if (value === undefined || value === "") {
    throw new RouteError(500, "internal", `${name} is not configured for this function`);
  }
  return value;
}

function success(body: JsonObject): Response {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { "content-type": "application/json" },
  });
}

function failure(status: number, code: string, message: string): Response {
  return new Response(JSON.stringify({ error: { code, message } }), {
    status,
    headers: { "content-type": "application/json" },
  });
}
