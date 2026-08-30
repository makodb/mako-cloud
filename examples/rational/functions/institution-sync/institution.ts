/**
 * A simulated bank.
 *
 * Rational is a sample, so it cannot connect to a real aggregator; but the
 * thing worth exercising is not the aggregator, it is everything around it --
 * a schedule that fires, a function that writes with a service credential,
 * transactions that must not double when a sync runs twice. So the
 * institution is deterministic: the same connection asked for the same window
 * always answers the same statement, which is what makes "it ran twice"
 * testable at all.
 *
 * It lives on this function's own routes (`/institution/statement`) so the
 * function calls it the way it would call a real one, over HTTP it does not
 * privilege.
 */

export interface StatementEntry {
  /** Stable at the institution: what makes a repeated sync idempotent. */
  readonly externalId: string;
  readonly date: string;
  readonly amount: number;
  readonly description: string;
}

const MERCHANTS = [
  "CORNER MARKET",
  "STREAMING SERVICE",
  "CITY TRANSIT",
  "PHARMACY",
  "COFFEE BAR",
  "HARDWARE STORE",
  "ELECTRIC UTILITY",
] as const;

/** A small deterministic hash, so the same inputs give the same statement. */
function seedOf(text: string): number {
  let seed = 2_166_136_261;
  for (let index = 0; index < text.length; index += 1) {
    seed ^= text.charCodeAt(index);
    seed = Math.imul(seed, 16_777_619) >>> 0;
  }
  return seed;
}

function nextRandom(seed: number): [number, number] {
  const next = Math.imul(seed ^ (seed >>> 15), 2_246_822_507) >>> 0;
  return [next, next / 4_294_967_296];
}

/**
 * The statement for one account over the days ending at `through`. Entries
 * are dated within the window and carry ids stable for that (account, day,
 * index), so asking again -- for an overlapping window, or the same one --
 * returns the same entries with the same ids.
 */
export function statement(
  accountExternalId: string,
  through: string,
  days: number,
): readonly StatementEntry[] {
  const entries: StatementEntry[] = [];
  const end = Date.parse(`${through}T00:00:00Z`);
  for (let offset = days - 1; offset >= 0; offset -= 1) {
    const date = new Date(end - offset * 86_400_000).toISOString().slice(0, 10);
    let seed = seedOf(`${accountExternalId}:${date}`);
    let value: number;
    [seed, value] = nextRandom(seed);
    // Roughly two days in five have no activity at all.
    if (value < 0.4) continue;
    const count = value < 0.8 ? 1 : 2;
    for (let index = 0; index < count; index += 1) {
      [seed, value] = nextRandom(seed);
      const merchant = MERCHANTS[Math.floor(value * MERCHANTS.length)] ?? MERCHANTS[0];
      [seed, value] = nextRandom(seed);
      const amount = -(100 + Math.floor(value * 9_900));
      entries.push({
        externalId: `${accountExternalId}-${date}-${index}`,
        date,
        amount,
        description: `${merchant} ${date.replaceAll("-", "")}`,
      });
    }
  }
  // A salary on the first of each month, so a household has income too.
  for (let offset = days - 1; offset >= 0; offset -= 1) {
    const date = new Date(end - offset * 86_400_000).toISOString().slice(0, 10);
    if (!date.endsWith("-01")) continue;
    entries.push({
      externalId: `${accountExternalId}-${date}-salary`,
      date,
      amount: 250_000,
      description: `SALARY ${date.slice(0, 7)}`,
    });
  }
  return entries.sort((left, right) => left.date.localeCompare(right.date));
}

/**
 * `.` rather than `:`: a document id holding a character
 * `encodeURIComponent` escapes cannot be written from a function (findings
 * log #12), and this id is written from one on every sync.
 */
export function transactionId(accountId: string, externalId: string): string {
  return `txn_${accountId}.${externalId}`.replaceAll(/[^A-Za-z0-9_.-]/gu, "-").slice(0, 128);
}
