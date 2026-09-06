import type { Transaction } from "../model/types.js";
import { memoizeLast } from "./memo.js";

/**
 * How many transactions carry each tag, by tag id.
 *
 * A tag with no transactions is not in the map -- the tags page reads zero
 * from its absence -- and every transaction counts, hidden and transfer legs
 * included: the question on the tags page is what a tag is attached to, not
 * what it cost, and a tag that only marks transfers is still in use.
 */
export function tagUsage(transactions: readonly Transaction[]): ReadonlyMap<string, number> {
  const usage = new Map<string, number>();
  for (const transaction of transactions) {
    for (const tag of new Set(transaction.tags)) {
      usage.set(tag, (usage.get(tag) ?? 0) + 1);
    }
  }
  return usage;
}

export const selectTagUsage = memoizeLast(tagUsage);
