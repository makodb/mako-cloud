/**
 * Shared between the application and its edge functions -- see the note in
 * `rules.ts` for why this file imports nothing at all.
 *
 * Who a transaction was with.
 *
 * A merchant is a taxonomy entry with a display name and the normalized
 * descriptions that mean it. A transaction's merchant is the one it was
 * filed under by hand or by a rule; failing that, the one whose patterns
 * include its normalized description; failing that, a cleaned-up reading of
 * the statement text, so a list of transactions reads "Corner Grocer" rather
 * than "SQ *CORNER GROCER 0412" even before anybody has named anything.
 *
 * Normalization lives in `recurrences.ts`, which this module cannot import,
 * so the caller passes it in. Transactions carry `normalized_description`
 * already; the function is for the ones that do not.
 */

/** What resolution reads of a merchant document. */
export interface MerchantLike {
  readonly id: string;
  readonly name: string;
  readonly patterns?: readonly string[];
}

/** What resolution reads of a transaction. */
export interface MerchantSubject {
  readonly merchant_id?: string;
  readonly description: string;
  readonly normalized_description?: string;
}

export interface ResolvedMerchant {
  /** The merchant document, or null when the name is only a cleaned description. */
  readonly id: string | null;
  readonly name: string;
}

/**
 * Tokens a card processor puts in front of the merchant's own name. They
 * name the processor, not the shop, and the person never thinks of the
 * grocer as "Sq".
 */
const PROCESSOR_PREFIXES: ReadonlySet<string> = new Set([
  "sq",
  "tst",
  "pp",
  "paypal",
  "sp",
  "pos",
  "chkcard",
  "checkcard",
  "dbt",
  "pur",
  "purchase",
  "ach",
  "web",
  "tel",
  "ppd",
  "ccd",
]);

/**
 * A normalized description as a name: processor prefixes dropped, stray
 * single letters (the "s" of a possessive whose apostrophe normalization
 * removed) dropped, and each word capitalized. Returns the empty string when
 * nothing survives, so the caller can fall back to the raw text.
 */
export function cleanDescription(normalized: string): string {
  const words = normalized.split(" ").filter((word) => word !== "");
  let start = 0;
  while (start < words.length && PROCESSOR_PREFIXES.has(words[start] ?? "")) start += 1;
  // A description that is nothing but a processor's name keeps it: "PayPal"
  // is a better name than nothing.
  const kept = start === words.length ? words : words.slice(start);
  return kept
    .filter((word) => word.length > 1)
    .map((word) => `${word.charAt(0).toUpperCase()}${word.slice(1)}`)
    .join(" ");
}

export function resolveMerchant(
  transaction: MerchantSubject,
  merchants: ReadonlyArray<MerchantLike>,
  normalize: (text: string) => string,
): ResolvedMerchant {
  if (transaction.merchant_id !== undefined && transaction.merchant_id !== "") {
    const chosen = merchants.find((merchant) => merchant.id === transaction.merchant_id);
    // A merchant id pointing at a deleted document is still a choice; the
    // cleaned description stands in for the name until it is refiled.
    if (chosen !== undefined) return { id: chosen.id, name: chosen.name };
  }
  const normalized = transaction.normalized_description ?? normalize(transaction.description);
  if (normalized !== "") {
    const byPattern = merchants.find((merchant) => merchant.patterns?.includes(normalized));
    if (byPattern !== undefined) return { id: byPattern.id, name: byPattern.name };
  }
  const cleaned = cleanDescription(normalized);
  return { id: null, name: cleaned === "" ? transaction.description.trim() : cleaned };
}

export interface MergePlan {
  /** What the surviving merchant becomes: its patterns plus the other's. */
  readonly winnerPatch: { readonly patterns: readonly string[] };
  /** What every transaction of the merged-away merchant is refiled to. */
  readonly transactionPatch: { readonly merchant_id: string };
}

/**
 * Merging two merchants into one: the winner keeps its name and takes the
 * loser's patterns, so the descriptions that used to resolve to the loser
 * resolve to the winner from now on; the loser's transactions are refiled;
 * the caller then deletes the loser.
 */
export function mergePlan(loser: MerchantLike, winner: MerchantLike): MergePlan {
  const patterns = [...new Set([...(winner.patterns ?? []), ...(loser.patterns ?? [])])];
  return {
    winnerPatch: { patterns },
    transactionPatch: { merchant_id: winner.id },
  };
}
