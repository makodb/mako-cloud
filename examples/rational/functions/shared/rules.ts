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

export type RuleDirection = "expense" | "income";

/**
 * What a rule reads of a transaction. `normalized_description` is what the
 * exact-text condition compares against; transactions store it, and a caller
 * that has only the raw description gets a lower-cased comparison instead,
 * because this module cannot import the normalizer.
 */
export interface RuleSubject {
  readonly description: string;
  readonly normalized_description?: string;
  readonly amount: number;
  readonly account_id: string;
  readonly category_id?: string;
  readonly merchant_id?: string;
  readonly hidden?: boolean;
  readonly reviewed?: boolean;
  readonly id?: string;
}

export interface RuleLike {
  readonly id: string;
  readonly name: string;
  readonly match: {
    readonly description_contains?: string;
    readonly description_equals?: string;
    readonly merchant_id?: string;
    readonly category_id?: string;
    readonly direction?: RuleDirection;
    readonly amount_min?: number;
    readonly amount_max?: number;
    readonly account_id?: string;
  };
  readonly set_category_id?: string;
  readonly set_merchant_id?: string;
  readonly add_tags: readonly string[];
  readonly hide?: boolean;
  readonly mark_reviewed?: boolean;
  readonly priority: number;
  readonly enabled: boolean;
}

export interface RuleOutcome {
  readonly rule: RuleLike;
  readonly categoryId?: string;
  readonly merchantId?: string;
  readonly tags: readonly string[];
  readonly hide?: boolean;
  readonly markReviewed?: boolean;
}

function stated(value: string | undefined): value is string {
  return value !== undefined && value.trim() !== "";
}

/**
 * Whether the rule has any condition at all. Every condition counts, so a
 * rule that only says "income" or only names a merchant is a rule; one that
 * says nothing would match the whole household.
 */
export function ruleStatesSomething(rule: RuleLike): boolean {
  const { match } = rule;
  return (
    stated(match.description_contains) ||
    stated(match.description_equals) ||
    stated(match.merchant_id) ||
    stated(match.category_id) ||
    match.direction !== undefined ||
    match.amount_min !== undefined ||
    match.amount_max !== undefined ||
    stated(match.account_id)
  );
}

export function ruleMatches(rule: RuleLike, transaction: RuleSubject): boolean {
  if (!rule.enabled || !ruleStatesSomething(rule)) return false;
  const { match } = rule;
  if (stated(match.description_contains)) {
    if (
      !transaction.description
        .toLowerCase()
        .includes(match.description_contains.toLowerCase().trim())
    ) {
      return false;
    }
  }
  // Exact text is compared against the normalized description -- the form
  // that two charges from one merchant share once the bank's digits and
  // punctuation are gone -- so "equals netflix com" holds across months.
  if (stated(match.description_equals)) {
    const subject =
      transaction.normalized_description ?? transaction.description.toLowerCase().trim();
    if (subject !== match.description_equals.toLowerCase().trim()) return false;
  }
  if (stated(match.merchant_id) && transaction.merchant_id !== match.merchant_id) return false;
  if (stated(match.category_id) && transaction.category_id !== match.category_id) return false;
  // Zero is neither: a rule about money leaving or arriving is not about
  // a placeholder that moved nothing.
  if (match.direction === "expense" && !(transaction.amount < 0)) return false;
  if (match.direction === "income" && !(transaction.amount > 0)) return false;
  // The range is over the amount as stored: an expense is negative, so
  // "between -50.00 and -10.00" is what a person means by "small purchases".
  if (match.amount_min !== undefined && transaction.amount < match.amount_min) return false;
  if (match.amount_max !== undefined && transaction.amount > match.amount_max) return false;
  if (stated(match.account_id) && transaction.account_id !== match.account_id) return false;
  return true;
}

/** Lowest priority number first; ties broken by id so every device agrees. */
export function sortRules<T extends RuleLike>(rules: readonly T[]): T[] {
  return [...rules].sort(
    (left, right) => left.priority - right.priority || left.id.localeCompare(right.id),
  );
}

/** What a rule would set, without the rule itself. */
function actionsOf(rule: RuleLike): Omit<RuleOutcome, "rule"> {
  return {
    ...(stated(rule.set_category_id) ? { categoryId: rule.set_category_id } : {}),
    ...(stated(rule.set_merchant_id) ? { merchantId: rule.set_merchant_id } : {}),
    tags: rule.add_tags,
    ...(rule.hide === true ? { hide: true } : {}),
    ...(rule.mark_reviewed === true ? { markReviewed: true } : {}),
  };
}

/** The first rule that matches, with what it would set. */
export function applyRules(
  rules: readonly RuleLike[],
  transaction: RuleSubject,
): RuleOutcome | null {
  for (const rule of sortRules(rules)) {
    if (!ruleMatches(rule, transaction)) continue;
    return { rule, ...actionsOf(rule) };
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
 * Whether applying the rule to a transaction it matches would change it. The
 * category is the one action a person's own filing wins over, so it counts
 * only when the transaction has none or a different one; a merchant it lacks,
 * hiding one that is shown, or reviewing one that is not all count. Tags do
 * not: the subject does not carry them, and a rule that only tags is applied
 * with whatever else it does.
 */
export function wouldChange(rule: RuleLike, transaction: RuleSubject): boolean {
  const actions = actionsOf(rule);
  return (
    (actions.categoryId !== undefined && transaction.category_id !== actions.categoryId) ||
    (actions.merchantId !== undefined && transaction.merchant_id !== actions.merchantId) ||
    (actions.hide === true && transaction.hidden !== true) ||
    (actions.markReviewed === true && transaction.reviewed !== true)
  );
}

/**
 * The transactions a rule would change: matching, and not already as the rule
 * would leave them. Applying a rule to what it already agrees with would
 * rewrite documents for no change and push them all.
 */
export function pendingRecategorization(
  rule: RuleLike,
  transactions: readonly RuleSubject[],
): readonly RuleSubject[] {
  return transactions.filter(
    (transaction) => ruleMatches(rule, transaction) && wouldChange(rule, transaction),
  );
}

/**
 * Consecutive priorities for a list already in the order wanted: 10, 20, 30.
 * Tens rather than ones so a rule inserted by hand between two has room; the
 * next reorder closes the gaps again.
 */
export function reprioritize(
  rulesInOrder: ReadonlyArray<{ readonly id: string }>,
): Array<{ readonly id: string; readonly priority: number }> {
  return rulesInOrder.map((rule, index) => ({ id: rule.id, priority: (index + 1) * 10 }));
}

/**
 * The rules in their sorted order with one moved a step up or down. Sorting
 * first is what makes "up" mean the same thing on every device: two rules
 * with equal priority are ordered by id, and a move swaps neighbours of that
 * order, not of the array as it happened to arrive. A rule already at the
 * edge, or one not in the list, leaves the order as it is; the caller
 * reprioritizes the result.
 */
export function moveRule<T extends RuleLike>(
  rules: readonly T[],
  id: string,
  direction: "up" | "down",
): T[] {
  const ordered = sortRules(rules);
  const index = ordered.findIndex((rule) => rule.id === id);
  if (index === -1) return ordered;
  const target = direction === "up" ? index - 1 : index + 1;
  if (target < 0 || target >= ordered.length) return ordered;
  const moved = ordered[index];
  const neighbour = ordered[target];
  if (moved === undefined || neighbour === undefined) return ordered;
  ordered[index] = neighbour;
  ordered[target] = moved;
  return ordered;
}
