/** Identifier helpers shared by the app, the seed, and the tests. */

const ALPHABET = "abcdefghijklmnopqrstuvwxyz0123456789";

export function randomId(prefix: string, length = 16): string {
  const bytes = new Uint8Array(length);
  globalThis.crypto.getRandomValues(bytes);
  let suffix = "";
  for (const byte of bytes) {
    suffix += ALPHABET[byte % ALPHABET.length];
  }
  return `${prefix}_${suffix}`;
}

/** Budgets are one per category per month: the id is derived, never random. */
export function budgetId(householdId: string, categoryId: string, month: string): string {
  return `${householdId}:${categoryId}:${month}`;
}

/**
 * A membership's id. The separator is `.` rather than `:` on purpose: the
 * households function writes memberships through the service document route,
 * whose path segment the edge SDK percent-encodes while the route compares it
 * to the body id byte for byte (findings log #7c), so an id holding any
 * character `encodeURIComponent` escapes cannot be written from a function.
 */
export function membershipId(householdId: string, userId: string): string {
  return `${householdId}.${userId}`;
}

/**
 * A pending invitation's id. The invitee may not have an account yet, so the
 * id is derived from the household and the lowercased address; the digest
 * keeps `@` and `.` — and the address itself — out of the request path.
 */
export function invitationId(householdId: string, emailDigest: string): string {
  return `${householdId}.invite.${emailDigest}`;
}

/** Lowercase, trim, and hash an address the way the households function does. */
export async function emailDigest(email: string): Promise<string> {
  const bytes = new TextEncoder().encode(email.trim().toLowerCase());
  const digest = await globalThis.crypto.subtle.digest("SHA-256", bytes);
  return Array.from(new Uint8Array(digest).slice(0, 16))
    .map((byte) => byte.toString(16).padStart(2, "0"))
    .join("");
}

export function snapshotId(householdId: string, date: string): string {
  return `${householdId}:${date}`;
}

/** RxDB database names are restricted; household ids are not. */
export function databaseName(prefix: string, key: string): string {
  const safe = key.toLowerCase().replaceAll(/[^a-z0-9_$()+/-]/gu, "-");
  return `${prefix}-${safe}`;
}
