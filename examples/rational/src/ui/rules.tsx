import { type FormEvent, useEffect, useRef, useState } from "react";

import { moveRule, reprioritize, wouldChange } from "../../functions/shared/rules.js";
import type { RationalApp } from "../data/rational.js";
import type { ScopeSession } from "../data/scope.js";
import {
  type HouseholdWrites,
  type Patch,
  type RuleInput,
  ValidationError,
} from "../data/writes.js";
import {
  type HouseholdCollectionId,
  isCategory,
  isMerchant,
  isTag,
  type Rule,
  type RuleDirection,
  type TaxonomyEntry,
  type Transaction,
} from "../model/types.js";
import { amountToText, formatMinorUnits, parseAmount } from "../selectors/money.js";
import {
  applyRules,
  countMatches,
  pendingRecategorization,
  ruleMatches,
  sortRules,
} from "../selectors/rules.js";
import { normalizeDescription } from "../selectors/transactions.js";
import { useQuery } from "./hooks.js";
import "./styles/taxonomy.css";

/**
 * Categorization rules.
 *
 * A rule says how a transaction should be filed, and the list says how many
 * of the household's transactions each would touch before it touches any: a
 * rule is written against real data, so the count is the feedback that makes
 * it writable. Rules run in their listed order -- the first that matches a
 * transaction is the one that files it -- and the order is the `priority`
 * on each document, rewritten as consecutive tens whenever a rule moves so
 * every device sorts the list the same way. Applying is a separate, explicit
 * action: one rule to everything it matches, or every rule in order, as the
 * nightly job runs them.
 */
type TextMode = "contains" | "equals";

interface RuleDraft {
  /** The rule being edited, or null for a new one. */
  readonly id: string | null;
  /** Changes whenever the draft is replaced, so the uncontrolled form re-reads its defaults. */
  readonly nonce: number;
  readonly name: string;
  readonly textMode: TextMode;
  readonly text: string;
  readonly merchantId: string;
  readonly categoryId: string;
  readonly direction: RuleDirection | "";
  readonly amountMin: string;
  readonly amountMax: string;
  readonly accountId: string;
  readonly setCategoryId: string;
  readonly setMerchantId: string;
  readonly addTags: readonly string[];
  readonly hide: boolean;
  readonly markReviewed: boolean;
  readonly priority: number;
}

let drafts = 0;

function emptyDraft(priority: number): RuleDraft {
  drafts += 1;
  return {
    id: null,
    nonce: drafts,
    name: "",
    textMode: "contains",
    text: "",
    merchantId: "",
    categoryId: "",
    direction: "",
    amountMin: "",
    amountMax: "",
    accountId: "",
    setCategoryId: "",
    setMerchantId: "",
    addTags: [],
    hide: false,
    markReviewed: false,
    priority,
  };
}

function draftFromRule(rule: Rule, currency: string): RuleDraft {
  const { match } = rule;
  const equals = match.description_equals !== undefined && match.description_equals !== "";
  return {
    ...emptyDraft(rule.priority),
    id: rule.id,
    name: rule.name,
    textMode: equals ? "equals" : "contains",
    text: equals ? (match.description_equals ?? "") : (match.description_contains ?? ""),
    merchantId: match.merchant_id ?? "",
    categoryId: match.category_id ?? "",
    direction: match.direction ?? "",
    amountMin: match.amount_min === undefined ? "" : amountToText(match.amount_min, currency),
    amountMax: match.amount_max === undefined ? "" : amountToText(match.amount_max, currency),
    accountId: match.account_id ?? "",
    setCategoryId: rule.set_category_id ?? "",
    setMerchantId: rule.set_merchant_id ?? "",
    addTags: rule.add_tags,
    hide: rule.hide === true,
    markReviewed: rule.mark_reviewed === true,
  };
}

/**
 * A rule started from a transaction: its statement text, normalized so the
 * bank's reference numbers on next month's charge do not matter, on its
 * account. The category is left for the person to choose -- the transaction
 * was probably brought here because its filing was wrong.
 */
function draftFromTransaction(transaction: Transaction, priority: number): RuleDraft {
  const normalized =
    transaction.normalized_description ?? normalizeDescription(transaction.description);
  return {
    ...emptyDraft(priority),
    name: `Rule for ${transaction.description.trim()}`,
    textMode: "contains",
    text: normalized,
    accountId: transaction.account_id,
  };
}

/** `?from=<id>` after the route path; the router ignores the query on this page. */
function fromParameter(hash: string): string | null {
  const query = hash.split("?")[1];
  if (query === undefined) return null;
  const from = new URLSearchParams(query).get("from");
  return from === null || from === "" ? null : from;
}

function stated(value: string | undefined): value is string {
  return value !== undefined && value.trim() !== "";
}

/** What the editor's form says, as the write layer wants it. */
function ruleInputFrom(data: FormData, currency: string, priority: number): RuleInput {
  const text = String(data.get("text") ?? "").trim();
  const mode: TextMode = data.get("text_mode") === "equals" ? "equals" : "contains";
  const minimum = String(data.get("amount_min") ?? "").trim();
  const maximum = String(data.get("amount_max") ?? "").trim();
  const direction = String(data.get("direction") ?? "");
  return {
    name: String(data.get("name") ?? "").trim(),
    match: {
      ...(text === ""
        ? {}
        : mode === "equals"
          ? { description_equals: text }
          : { description_contains: text }),
      merchant_id: String(data.get("merchant_id") ?? ""),
      category_id: String(data.get("category_id") ?? ""),
      ...(direction === "expense" || direction === "income" ? { direction } : {}),
      ...(minimum === "" ? {} : { amount_min: parseAmount(minimum, currency) }),
      ...(maximum === "" ? {} : { amount_max: parseAmount(maximum, currency) }),
      account_id: String(data.get("account_id") ?? ""),
    },
    set_category_id: String(data.get("set_category_id") ?? ""),
    set_merchant_id: String(data.get("set_merchant_id") ?? ""),
    add_tags: data.getAll("add_tags").map(String),
    hide: data.get("hide") === "on",
    mark_reviewed: data.get("mark_reviewed") === "on",
    priority,
  };
}

/**
 * The patch that makes an existing rule say what the editor says. It cleans
 * the conditions the way `createRule` does -- trimmed, the exact text
 * normalized, nothing stored for a blank -- because the engine compares
 * against stored values and a rule edited by hand must hold as well as one
 * just written.
 */
function rulePatchFrom(input: RuleInput): Patch<Rule> {
  if (input.name.trim() === "") throw new ValidationError("a rule needs a name");
  const match: Rule["match"] = {
    ...(stated(input.match.description_contains)
      ? { description_contains: input.match.description_contains.trim() }
      : {}),
    ...(stated(input.match.description_equals)
      ? { description_equals: normalizeDescription(input.match.description_equals) }
      : {}),
    ...(stated(input.match.merchant_id) ? { merchant_id: input.match.merchant_id } : {}),
    ...(stated(input.match.category_id) ? { category_id: input.match.category_id } : {}),
    ...(input.match.direction === undefined ? {} : { direction: input.match.direction }),
    ...(input.match.amount_min === undefined ? {} : { amount_min: input.match.amount_min }),
    ...(input.match.amount_max === undefined ? {} : { amount_max: input.match.amount_max }),
    ...(stated(input.match.account_id) ? { account_id: input.match.account_id } : {}),
  };
  if (Object.keys(match).length === 0) {
    throw new ValidationError("a rule needs at least one condition");
  }
  if (
    match.amount_min !== undefined &&
    match.amount_max !== undefined &&
    match.amount_min > match.amount_max
  ) {
    throw new ValidationError("the smallest amount must not be above the largest");
  }
  return {
    name: input.name.trim(),
    match,
    set_category_id: stated(input.set_category_id) ? input.set_category_id : null,
    set_merchant_id: stated(input.set_merchant_id) ? input.set_merchant_id : null,
    add_tags: [...(input.add_tags ?? [])],
    hide: input.hide === true ? true : null,
    mark_reviewed: input.mark_reviewed === true ? true : null,
  };
}

/** The field changes a rule makes to a transaction it files, with the rule's mark. */
function actionPatch(rule: Rule): Patch<Transaction> {
  return {
    ...(stated(rule.set_category_id) ? { category_id: rule.set_category_id } : {}),
    ...(stated(rule.set_merchant_id) ? { merchant_id: rule.set_merchant_id } : {}),
    ...(rule.hide === true ? { hidden: true } : {}),
    ...(rule.mark_reviewed === true ? { reviewed: true } : {}),
    rule_id: rule.id,
  };
}

function plural(count: number, noun: string): string {
  return `${count} ${count === 1 ? noun : `${noun}s`}`;
}

interface Names {
  readonly account: (id: string) => string;
  readonly category: (id: string) => string;
  readonly merchant: (id: string) => string;
  readonly tag: (id: string) => string;
}

/** The conditions as a person would say them, joined with "and". */
function describeConditions(rule: Rule, names: Names, currency: string): string {
  const { match } = rule;
  const parts: string[] = [];
  if (stated(match.description_contains)) {
    parts.push(`statement text contains “${match.description_contains.trim()}”`);
  }
  if (stated(match.description_equals))
    parts.push(`statement text is “${match.description_equals}”`);
  if (stated(match.merchant_id)) parts.push(`merchant is ${names.merchant(match.merchant_id)}`);
  if (stated(match.category_id)) parts.push(`category is ${names.category(match.category_id)}`);
  if (match.direction === "expense") parts.push("money goes out");
  if (match.direction === "income") parts.push("money comes in");
  if (match.amount_min !== undefined) {
    parts.push(`amount is at least ${formatMinorUnits(match.amount_min, currency)}`);
  }
  if (match.amount_max !== undefined) {
    parts.push(`amount is at most ${formatMinorUnits(match.amount_max, currency)}`);
  }
  if (stated(match.account_id)) parts.push(`account is ${names.account(match.account_id)}`);
  return parts.length === 0 ? "anything" : parts.join(" and ");
}

/** The actions as a person would say them, joined with commas. */
function describeActions(rule: Rule, names: Names): string {
  const parts: string[] = [];
  if (stated(rule.set_category_id))
    parts.push(`set category to ${names.category(rule.set_category_id)}`);
  if (stated(rule.set_merchant_id))
    parts.push(`set merchant to ${names.merchant(rule.set_merchant_id)}`);
  if (rule.add_tags.length > 0) parts.push(`add tags ${rule.add_tags.map(names.tag).join(", ")}`);
  if (rule.hide === true) parts.push("hide it");
  if (rule.mark_reviewed === true) parts.push("mark it reviewed");
  return parts.length === 0 ? "do nothing" : parts.join(", ");
}

export function RulesScreen({
  app,
  session,
  currency,
}: {
  app: RationalApp;
  session: ScopeSession<HouseholdCollectionId>;
  currency: string;
}) {
  const rules = useQuery(session.collection("rules")?.find() ?? null);
  const transactions = useQuery(session.collection("transactions")?.find() ?? null);
  const accounts = useQuery(
    session.collection("accounts")?.find({ sort: [{ name: "asc" }] }) ?? null,
  );
  const taxonomy = useQuery(
    session.collection("taxonomy")?.find({ sort: [{ name: "asc" }] }) ?? null,
  );
  const categories = taxonomy.filter(isCategory);
  const merchants = taxonomy.filter(isMerchant);
  const tags = taxonomy.filter(isTag);

  const [draft, setDraft] = useState<RuleDraft | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [applied, setApplied] = useState<string | null>(null);

  const ordered = sortRules(rules);
  const nextPriority = (ordered.at(-1)?.priority ?? 0) + 10;

  // The transactions panel links here with `?from=<id>` to start a rule from
  // a transaction. Read once, on mount; the editor opens as soon as the
  // transaction is on this device.
  const [from] = useState(() => fromParameter(window.location.hash));
  const prefilled = useRef(false);
  useEffect(() => {
    if (from === null || prefilled.current) return;
    const transaction = transactions.find((candidate) => candidate.id === from);
    if (transaction === undefined) return;
    prefilled.current = true;
    setDraft(draftFromTransaction(transaction, nextPriority));
  }, [from, transactions, nextPriority]);

  const nameOf = (entries: readonly TaxonomyEntry[]) => (id: string) =>
    entries.find((entry) => entry.id === id)?.name ?? "a deleted entry";
  const names: Names = {
    account: (id) => accounts.find((account) => account.id === id)?.name ?? "a deleted account",
    category: nameOf(categories),
    merchant: nameOf(merchants),
    tag: nameOf(tags),
  };

  /** Run one write, turning a refusal into the message under the heading. */
  const attempt = async (
    action: (writes: HouseholdWrites) => Promise<unknown>,
    fallback: string,
  ): Promise<boolean> => {
    setProblem(null);
    const writes = app.writes;
    if (writes === null) {
      setProblem("No household is open.");
      return false;
    }
    try {
      await action(writes);
      return true;
    } catch (caught) {
      setProblem(caught instanceof Error ? caught.message : fallback);
      return false;
    }
  };

  const save = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (draft === null) return;
    const data = new FormData(event.currentTarget);
    const saved = await attempt(async (writes) => {
      const input = ruleInputFrom(data, currency, draft.priority);
      if (draft.id === null) await writes.createRule(input);
      else await writes.updateRule(draft.id, rulePatchFrom(input));
    }, "The rule could not be saved.");
    if (saved) setDraft(null);
  };

  /**
   * One rule, to everything it matches: the field changes to the
   * transactions it would change, the tags to every match (a tag already
   * there is left alone), and the match count on the rule itself.
   */
  const apply = (rule: Rule) =>
    void attempt(async (writes) => {
      const matching = transactions.filter((transaction) => ruleMatches(rule, transaction));
      const pending = matching.filter((transaction) => wouldChange(rule, transaction));
      const changed = await writes.bulkPatchTransactions(
        pending.map((transaction) => transaction.id),
        actionPatch(rule),
      );
      const tagged = await writes.addTagsToTransactions(
        matching.map((transaction) => transaction.id),
        rule.add_tags,
      );
      await writes.updateRule(rule.id, { match_count: matching.length });
      setApplied(
        changed === 0 && tagged === 0
          ? `${rule.name} already agrees with every transaction it matches.`
          : `${rule.name} changed ${plural(changed, "transaction")}${
              tagged === 0 ? "" : ` and tagged ${tagged}`
            }.`,
      );
    }, "The rule could not be applied.");

  /**
   * Every enabled rule in order, the way the nightly job runs them: the first
   * rule that matches a transaction is the one that files it, so a rule
   * listed lower never overrides one listed higher.
   */
  const applyAll = () =>
    void attempt(async (writes) => {
      const enabled = ordered.filter((rule) => rule.enabled);
      const byRule = new Map<string, { rule: Rule; matched: string[]; pending: string[] }>(
        enabled.map((rule) => [rule.id, { rule, matched: [], pending: [] }]),
      );
      for (const transaction of transactions) {
        const outcome = applyRules(enabled, transaction);
        if (outcome === null) continue;
        const entry = byRule.get(outcome.rule.id);
        if (entry === undefined) continue;
        entry.matched.push(transaction.id);
        if (wouldChange(entry.rule, transaction)) entry.pending.push(transaction.id);
      }
      let changed = 0;
      for (const entry of byRule.values()) {
        changed += await writes.bulkPatchTransactions(entry.pending, actionPatch(entry.rule));
        await writes.addTagsToTransactions(entry.matched, entry.rule.add_tags);
        if (entry.rule.match_count !== entry.matched.length) {
          await writes.updateRule(entry.rule.id, { match_count: entry.matched.length });
        }
      }
      setApplied(
        changed === 0
          ? "Every rule already agrees with the transactions it files."
          : `The rules changed ${plural(changed, "transaction")}.`,
      );
    }, "The rules could not be applied.");

  const move = (rule: Rule, direction: "up" | "down") =>
    void attempt(async (writes) => {
      for (const { id, priority } of reprioritize(moveRule(rules, rule.id, direction))) {
        const current = rules.find((candidate) => candidate.id === id);
        if (current !== undefined && current.priority !== priority) {
          await writes.updateRule(id, { priority });
        }
      }
    }, "The rule could not be moved.");

  return (
    <section aria-labelledby="rules-title" data-testid="rules-screen">
      <div className="heading">
        <h1 id="rules-title">Rules</h1>
        <div className="taxonomy-inline">
          <button
            type="button"
            className="secondary"
            disabled={rules.length === 0}
            onClick={applyAll}
          >
            Apply all rules
          </button>
          <button type="button" onClick={() => setDraft(emptyDraft(nextPriority))}>
            New rule
          </button>
        </div>
      </div>
      {problem === null ? null : (
        <p className="notice error" role="alert">
          {problem}
        </p>
      )}
      {applied === null ? null : (
        <p className="taxonomy-status" role="status" data-testid="rule-applied">
          {applied}
        </p>
      )}

      {draft === null ? null : (
        <form
          key={`${draft.id ?? "new"}:${draft.nonce}`}
          className="editor"
          aria-label={draft.id === null ? "New rule" : "Edit rule"}
          onSubmit={(event) => void save(event)}
        >
          <div className="grid">
            <label>
              Name
              <input name="name" required maxLength={200} defaultValue={draft.name} />
            </label>
          </div>
          <fieldset>
            <legend>Conditions</legend>
            <div className="grid">
              <label>
                Statement text
                <input name="text" maxLength={500} defaultValue={draft.text} />
              </label>
              <div className="rule-text-mode" role="radiogroup" aria-label="Text match">
                <label className="chip-option">
                  <input
                    type="radio"
                    name="text_mode"
                    value="contains"
                    defaultChecked={draft.textMode === "contains"}
                  />
                  contains
                </label>
                <label className="chip-option">
                  <input
                    type="radio"
                    name="text_mode"
                    value="equals"
                    defaultChecked={draft.textMode === "equals"}
                  />
                  equals exactly
                </label>
              </div>
              <label>
                Merchant
                <select name="merchant_id" defaultValue={draft.merchantId}>
                  <option value="">Any merchant</option>
                  {merchants.map((merchant) => (
                    <option key={merchant.id} value={merchant.id}>
                      {merchant.name}
                    </option>
                  ))}
                </select>
              </label>
              <label>
                Category
                <select name="category_id" defaultValue={draft.categoryId}>
                  <option value="">Any category</option>
                  {categories.map((category) => (
                    <option key={category.id} value={category.id}>
                      {category.name}
                    </option>
                  ))}
                </select>
              </label>
              <label>
                Direction
                <select name="direction" defaultValue={draft.direction}>
                  <option value="">Any direction</option>
                  <option value="expense">Spending</option>
                  <option value="income">Income</option>
                </select>
              </label>
              <label>
                Amount at least
                <input
                  name="amount_min"
                  inputMode="decimal"
                  placeholder={amountToText(-5_000, currency)}
                  defaultValue={draft.amountMin}
                />
              </label>
              <label>
                Amount at most
                <input
                  name="amount_max"
                  inputMode="decimal"
                  placeholder={amountToText(-100, currency)}
                  defaultValue={draft.amountMax}
                />
              </label>
              <label>
                Account
                <select name="account_id" defaultValue={draft.accountId}>
                  <option value="">Any account</option>
                  {accounts.map((account) => (
                    <option key={account.id} value={account.id}>
                      {account.name}
                    </option>
                  ))}
                </select>
              </label>
            </div>
            <p className="hint">
              Amounts are as stored: spending is negative, so “at most -10.00” means purchases of
              ten or more.
            </p>
          </fieldset>
          <fieldset>
            <legend>Actions</legend>
            <div className="grid">
              <label>
                Set category
                <select name="set_category_id" defaultValue={draft.setCategoryId}>
                  <option value="">Leave the category</option>
                  {categories.map((category) => (
                    <option key={category.id} value={category.id}>
                      {category.name}
                    </option>
                  ))}
                </select>
              </label>
              <label>
                Set merchant
                <select name="set_merchant_id" defaultValue={draft.setMerchantId}>
                  <option value="">Leave the merchant</option>
                  {merchants.map((merchant) => (
                    <option key={merchant.id} value={merchant.id}>
                      {merchant.name}
                    </option>
                  ))}
                </select>
              </label>
            </div>
            {tags.length === 0 ? null : (
              <fieldset className="rule-tags">
                <legend>Add tags</legend>
                {tags.map((tag) => (
                  <label key={tag.id} className="chip-option">
                    <input
                      type="checkbox"
                      name="add_tags"
                      value={tag.id}
                      defaultChecked={draft.addTags.includes(tag.id)}
                    />
                    {tag.name}
                  </label>
                ))}
              </fieldset>
            )}
            <div className="rule-flags">
              <label className="chip-option">
                <input type="checkbox" name="hide" defaultChecked={draft.hide} />
                Hide from budgets and reports
              </label>
              <label className="chip-option">
                <input type="checkbox" name="mark_reviewed" defaultChecked={draft.markReviewed} />
                Mark reviewed
              </label>
            </div>
          </fieldset>
          <div className="actions">
            <button type="submit">Save rule</button>
            <button type="button" className="secondary" onClick={() => setDraft(null)}>
              Cancel
            </button>
          </div>
        </form>
      )}

      <table className="data-table rules-table" aria-label="Rules">
        <thead>
          <tr>
            <th scope="col">Order</th>
            <th scope="col">Rule</th>
            <th scope="col">Matches</th>
            <th scope="col">Would touch</th>
            <th scope="col">
              <span className="visually-hidden">Actions</span>
            </th>
          </tr>
        </thead>
        <tbody>
          {ordered.length === 0 ? (
            <tr>
              <td colSpan={5} className="empty">
                No rules yet.
              </td>
            </tr>
          ) : null}
          {ordered.map((rule, index) => (
            <tr
              key={rule.id}
              data-testid={`rule-${rule.id}`}
              data-name={rule.name}
              className={rule.enabled ? undefined : "muted"}
            >
              <td data-testid="order">{index + 1}</td>
              <th scope="row">
                <span data-testid="name">{rule.name}</span>
                {rule.enabled ? null : <span className="chip">disabled</span>}
                <span className="rule-sentence" data-testid="sentence">
                  When {describeConditions(rule, names, currency)}, {describeActions(rule, names)}.
                </span>
              </th>
              <td data-testid="match-count">{countMatches(rule, transactions)}</td>
              <td data-testid="pending">{pendingRecategorization(rule, transactions).length}</td>
              <td className="actions">
                <button
                  type="button"
                  className="link"
                  disabled={index === 0}
                  onClick={() => move(rule, "up")}
                >
                  Move up
                </button>
                <button
                  type="button"
                  className="link"
                  disabled={index >= ordered.length - 1}
                  onClick={() => move(rule, "down")}
                >
                  Move down
                </button>
                <button type="button" className="link" onClick={() => apply(rule)}>
                  Apply
                </button>
                <button
                  type="button"
                  className="link"
                  onClick={() =>
                    void attempt(
                      (writes) => writes.updateRule(rule.id, { enabled: !rule.enabled }),
                      "The rule could not be changed.",
                    )
                  }
                >
                  {rule.enabled ? "Disable" : "Enable"}
                </button>
                <button
                  type="button"
                  className="link"
                  onClick={() => setDraft(draftFromRule(rule, currency))}
                >
                  Edit
                </button>
                <button
                  type="button"
                  className="link"
                  onClick={() =>
                    void attempt(
                      (writes) => writes.deleteRule(rule.id),
                      "The rule could not be deleted.",
                    )
                  }
                >
                  Delete
                </button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </section>
  );
}
