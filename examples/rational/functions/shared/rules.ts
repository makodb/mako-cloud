/**
 * Shared between the application and its edge functions.
 *
 * A function bundle is a directory: nothing outside it is uploaded, so a
 * module both sides use has to be copied into each function's bundle at
 * deploy time (`scripts/bootstrap.mjs` does that, rewriting the function's
 * `../shared/` import to `./shared/` as it goes). To survive the copy this
 * module must import nothing at all -- the browser build wants `.js`
 * specifiers and Deno wants `.ts`, and no single import satisfies both. So
 * the types it needs are structural and declared here.
 */

/** What a rule reads of a transaction. */
export interface RuleSubject {
  readonly description: string;
  readonly amount: number;
  readonly account_id: string;
  readonly category_id?: string;
  readonly id?: string;
}

export interface RuleLike {
  readonly id: string;
  readonly name: string;
  readonly match: {
    readonly description_contains?: string;
    readonly amount_min?: number;
    readonly amount_max?: number;
    readonly account_id?: string;
  };
  readonly set_category_id?: string;
  readonly add_tags: readonly string[];
  readonly priority: number;
  readonly enabled: boolean;
}

export interface RuleOutcome {
  readonly rule: RuleLike;
  readonly categoryId?: string;
  readonly tags: readonly string[];
}

export function ruleStatesSomething(rule: RuleLike): boolean {
  const { description_contains, amount_min, amount_max, account_id } = rule.match;
  return (
    (description_contains !== undefined && description_contains.trim() !== "") ||
    amount_min !== undefined ||
    amount_max !== undefined ||
    (account_id !== undefined && account_id !== "")
  );
}

export function ruleMatches(rule: RuleLike, transaction: RuleSubject): boolean {
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
export function sortRules(rules: readonly RuleLike[]): RuleLike[] {
  return [...rules].sort(
    (left, right) => left.priority - right.priority || left.id.localeCompare(right.id),
  );
}

/** The first rule that matches, with what it would set. */
export function applyRules(
  rules: readonly RuleLike[],
  transaction: RuleSubject,
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
export function countMatches(rule: RuleLike, transactions: readonly RuleSubject[]): number {
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
  rule: RuleLike,
  transactions: readonly RuleSubject[],
): readonly RuleSubject[] {
  return transactions.filter(
    (transaction) =>
      ruleMatches(rule, transaction) &&
      (rule.set_category_id ?? "") !== "" &&
      transaction.category_id !== rule.set_category_id,
  );
}
