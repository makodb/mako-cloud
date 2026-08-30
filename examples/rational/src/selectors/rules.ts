import type { Rule, Transaction } from "../model/types.js";

/**
 * Categorization rules, applied on the device.
 *
 * A rule is a description substring, an amount range, an account, or any
 * combination; the first enabled rule whose every stated condition holds
 * wins, in priority order. Rules run when a transaction is created and when
 * one is imported, and the transaction records which rule decided it, so a
 * person can see why something was filed where it was -- and change the rule
 * rather than every transaction.
 *
 * A rule that states nothing matches nothing. The alternative, a rule with no
 * conditions matching everything, turns an empty form into a household-wide
 * recategorization.
 */

export interface RuleMatchInput {
  readonly description: string;
  readonly amount: number;
  readonly account_id: string;
}

export interface RuleOutcome {
  readonly rule: Rule;
  readonly categoryId?: string;
  readonly tags: readonly string[];
}

export function ruleStatesSomething(rule: Rule): boolean {
  const { description_contains, amount_min, amount_max, account_id } = rule.match;
  return (
    (description_contains !== undefined && description_contains.trim() !== "") ||
    amount_min !== undefined ||
    amount_max !== undefined ||
    (account_id !== undefined && account_id !== "")
  );
}

export function ruleMatches(rule: Rule, transaction: RuleMatchInput): boolean {
  if (!rule.enabled || !ruleStatesSomething(rule)) return false;
  const { description_contains, amount_min, amount_max, account_id } = rule.match;
  if (description_contains !== undefined && description_contains.trim() !== "") {
    if (
      !transaction.description.toLowerCase().includes(description_contains.toLowerCase().trim())
    ) {
      return false;
    }
  }
  // The range is over the amount as stored: an expense is negative, so
  // "between -50.00 and -10.00" is what a person means by "small purchases".
  if (amount_min !== undefined && transaction.amount < amount_min) return false;
  if (amount_max !== undefined && transaction.amount > amount_max) return false;
  if (account_id !== undefined && account_id !== "" && transaction.account_id !== account_id) {
    return false;
  }
  return true;
}

/** Lowest priority number first; ties broken by id so every device agrees. */
export function sortRules(rules: readonly Rule[]): Rule[] {
  return [...rules].sort(
    (left, right) => left.priority - right.priority || left.id.localeCompare(right.id),
  );
}

/** The first rule that matches, with what it would set. */
export function applyRules(
  rules: readonly Rule[],
  transaction: RuleMatchInput,
): RuleOutcome | null {
  for (const rule of sortRules(rules)) {
    if (!ruleMatches(rule, transaction)) continue;
    return {
      rule,
      ...(rule.set_category_id === undefined || rule.set_category_id === ""
        ? {}
        : { categoryId: rule.set_category_id }),
      tags: rule.add_tags,
    };
  }
  return null;
}

/**
 * How many stored transactions a rule would match, for the rule editor. It is
 * a count and not an action: a person writing a rule wants to know what it
 * would touch before it touches anything.
 */
export function countMatches(rule: Rule, transactions: readonly Transaction[]): number {
  let matched = 0;
  for (const transaction of transactions) {
    if (ruleMatches(rule, transaction)) matched += 1;
  }
  return matched;
}

/**
 * The transactions a rule would recategorize: matching, and not already
 * filed where the rule would file them. Applying a rule to what it already
 * agrees with would rewrite documents for no change and push them all.
 */
export function pendingRecategorization(
  rule: Rule,
  transactions: readonly Transaction[],
): readonly Transaction[] {
  return transactions.filter(
    (transaction) =>
      ruleMatches(rule, transaction) &&
      (rule.set_category_id ?? "") !== "" &&
      transaction.category_id !== rule.set_category_id,
  );
}
