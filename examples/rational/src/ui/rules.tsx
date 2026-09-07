import {
  Alert,
  AlertDescription,
  Badge,
  Button,
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
  Checkbox,
  Field,
  Input,
  Label,
  NativeSelect,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  cn,
} from "@mako-cloud/ui";
import { CircleAlert, CircleCheck, Play, Plus } from "lucide-react";
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

/** A row's small actions share one compact look. */
const ROW_ACTION = "h-7 px-2 text-xs";

/** A native radio, dressed only as far as its accent: the browser draws the dot. */
const RADIO = "size-4 shrink-0 rounded-full border-0 p-0 shadow-none accent-primary";

/**
 * A Radix checkbox is a button element; while the old stylesheet's button
 * padding is still loaded it would widen the box, so the box says it has none.
 */
const CHECKBOX = "p-0";

/** A group of fields inside the editor, with its legend as the group's name. */
const FIELDSET = "m-0 grid min-w-0 gap-4 rounded-lg border p-4";

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
    <section aria-labelledby="rules-title" data-testid="rules-screen" className="grid gap-6">
      <div className="flex flex-wrap items-start justify-between gap-4">
        <div className="grid gap-1">
          <h1 id="rules-title" className="text-2xl">
            Rules
          </h1>
          <p className="text-sm text-muted-foreground">
            Run in this order: the first rule that matches a transaction is the one that files it.
          </p>
        </div>
        <div className="flex flex-wrap gap-2">
          <Button variant="outline" disabled={rules.length === 0} onClick={applyAll}>
            <Play />
            Apply all rules
          </Button>
          <Button onClick={() => setDraft(emptyDraft(nextPriority))}>
            <Plus />
            New rule
          </Button>
        </div>
      </div>
      {problem === null ? null : (
        <Alert variant="destructive" role="alert">
          <CircleAlert />
          <AlertDescription>{problem}</AlertDescription>
        </Alert>
      )}
      {applied === null ? null : (
        <Alert variant="positive" role="status" data-testid="rule-applied">
          <CircleCheck />
          <AlertDescription>{applied}</AlertDescription>
        </Alert>
      )}

      {draft === null ? null : (
        <Card>
          <CardHeader>
            <CardTitle>{draft.id === null ? "New rule" : "Edit rule"}</CardTitle>
            <CardDescription>
              Every condition has to hold for the rule to match; every action is taken when it does.
            </CardDescription>
          </CardHeader>
          <CardContent>
            <form
              key={`${draft.id ?? "new"}:${draft.nonce}`}
              className="grid gap-5"
              aria-label={draft.id === null ? "New rule" : "Edit rule"}
              onSubmit={(event) => void save(event)}
            >
              <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
                <Field label="Name" htmlFor="rule-name">
                  <Input
                    id="rule-name"
                    name="name"
                    required
                    maxLength={200}
                    defaultValue={draft.name}
                  />
                </Field>
              </div>
              <fieldset className={FIELDSET}>
                <legend className="px-1 text-sm font-medium">Conditions</legend>
                <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
                  <Field label="Statement text" htmlFor="rule-text">
                    <Input id="rule-text" name="text" maxLength={500} defaultValue={draft.text} />
                  </Field>
                  <div
                    className="flex flex-wrap items-center gap-4 self-end pb-2.5"
                    role="radiogroup"
                    aria-label="Text match"
                  >
                    <Label htmlFor="rule-text-contains" className="font-normal">
                      <Input
                        id="rule-text-contains"
                        className={RADIO}
                        type="radio"
                        name="text_mode"
                        value="contains"
                        defaultChecked={draft.textMode === "contains"}
                      />
                      contains
                    </Label>
                    <Label htmlFor="rule-text-equals" className="font-normal">
                      <Input
                        id="rule-text-equals"
                        className={RADIO}
                        type="radio"
                        name="text_mode"
                        value="equals"
                        defaultChecked={draft.textMode === "equals"}
                      />
                      equals exactly
                    </Label>
                  </div>
                  <Field label="Merchant" htmlFor="rule-merchant">
                    <NativeSelect
                      id="rule-merchant"
                      name="merchant_id"
                      defaultValue={draft.merchantId}
                    >
                      <option value="">Any merchant</option>
                      {merchants.map((merchant) => (
                        <option key={merchant.id} value={merchant.id}>
                          {merchant.name}
                        </option>
                      ))}
                    </NativeSelect>
                  </Field>
                  <Field label="Category" htmlFor="rule-category">
                    <NativeSelect
                      id="rule-category"
                      name="category_id"
                      defaultValue={draft.categoryId}
                    >
                      <option value="">Any category</option>
                      {categories.map((category) => (
                        <option key={category.id} value={category.id}>
                          {category.name}
                        </option>
                      ))}
                    </NativeSelect>
                  </Field>
                  <Field label="Direction" htmlFor="rule-direction">
                    <NativeSelect
                      id="rule-direction"
                      name="direction"
                      defaultValue={draft.direction}
                    >
                      <option value="">Any direction</option>
                      <option value="expense">Spending</option>
                      <option value="income">Income</option>
                    </NativeSelect>
                  </Field>
                  <Field label="Amount at least" htmlFor="rule-amount-min">
                    <Input
                      id="rule-amount-min"
                      className="money"
                      name="amount_min"
                      inputMode="decimal"
                      placeholder={amountToText(-5_000, currency)}
                      defaultValue={draft.amountMin}
                    />
                  </Field>
                  <Field label="Amount at most" htmlFor="rule-amount-max">
                    <Input
                      id="rule-amount-max"
                      className="money"
                      name="amount_max"
                      inputMode="decimal"
                      placeholder={amountToText(-100, currency)}
                      defaultValue={draft.amountMax}
                    />
                  </Field>
                  <Field label="Account" htmlFor="rule-account">
                    <NativeSelect
                      id="rule-account"
                      name="account_id"
                      defaultValue={draft.accountId}
                    >
                      <option value="">Any account</option>
                      {accounts.map((account) => (
                        <option key={account.id} value={account.id}>
                          {account.name}
                        </option>
                      ))}
                    </NativeSelect>
                  </Field>
                </div>
                <p className="text-sm text-muted-foreground">
                  Amounts are as stored: spending is negative, so “at most -10.00” means purchases
                  of ten or more.
                </p>
              </fieldset>
              <fieldset className={FIELDSET}>
                <legend className="px-1 text-sm font-medium">Actions</legend>
                <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
                  <Field label="Set category" htmlFor="rule-set-category">
                    <NativeSelect
                      id="rule-set-category"
                      name="set_category_id"
                      defaultValue={draft.setCategoryId}
                    >
                      <option value="">Leave the category</option>
                      {categories.map((category) => (
                        <option key={category.id} value={category.id}>
                          {category.name}
                        </option>
                      ))}
                    </NativeSelect>
                  </Field>
                  <Field label="Set merchant" htmlFor="rule-set-merchant">
                    <NativeSelect
                      id="rule-set-merchant"
                      name="set_merchant_id"
                      defaultValue={draft.setMerchantId}
                    >
                      <option value="">Leave the merchant</option>
                      {merchants.map((merchant) => (
                        <option key={merchant.id} value={merchant.id}>
                          {merchant.name}
                        </option>
                      ))}
                    </NativeSelect>
                  </Field>
                </div>
                {tags.length === 0 ? null : (
                  <fieldset className="m-0 grid min-w-0 gap-3 rounded-md border border-dashed px-3 py-2.5">
                    <legend className="px-1 text-sm font-medium">Add tags</legend>
                    <div className="flex flex-wrap gap-x-5 gap-y-2">
                      {tags.map((tag) => (
                        <span key={tag.id} className="flex items-center gap-2">
                          <Checkbox
                            id={`rule-tag-${tag.id}`}
                            className={CHECKBOX}
                            name="add_tags"
                            value={tag.id}
                            defaultChecked={draft.addTags.includes(tag.id)}
                          />
                          <Label htmlFor={`rule-tag-${tag.id}`} className="font-normal">
                            {tag.name}
                          </Label>
                        </span>
                      ))}
                    </div>
                  </fieldset>
                )}
                <div className="flex flex-wrap gap-x-5 gap-y-2">
                  <span className="flex items-center gap-2">
                    <Checkbox
                      id="rule-hide"
                      className={CHECKBOX}
                      name="hide"
                      defaultChecked={draft.hide}
                    />
                    <Label htmlFor="rule-hide" className="font-normal">
                      Hide from budgets and reports
                    </Label>
                  </span>
                  <span className="flex items-center gap-2">
                    <Checkbox
                      id="rule-mark-reviewed"
                      className={CHECKBOX}
                      name="mark_reviewed"
                      defaultChecked={draft.markReviewed}
                    />
                    <Label htmlFor="rule-mark-reviewed" className="font-normal">
                      Mark reviewed
                    </Label>
                  </span>
                </div>
              </fieldset>
              <div className="flex flex-wrap gap-2">
                <Button type="submit">Save rule</Button>
                <Button variant="outline" onClick={() => setDraft(null)}>
                  Cancel
                </Button>
              </div>
            </form>
          </CardContent>
        </Card>
      )}

      <Card className="py-2">
        <CardContent className="px-2">
          <Table aria-label="Rules">
            <TableHeader>
              <TableRow className="hover:bg-transparent">
                <TableHead scope="col" className="w-16 text-right">
                  Order
                </TableHead>
                <TableHead scope="col">Rule</TableHead>
                <TableHead scope="col" className="text-right">
                  Matches
                </TableHead>
                <TableHead scope="col" className="text-right">
                  Would touch
                </TableHead>
                <TableHead scope="col">
                  <span className="sr-only">Actions</span>
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {ordered.length === 0 ? (
                <TableRow>
                  <TableCell colSpan={5} className="py-8 text-center text-muted-foreground">
                    No rules yet.
                  </TableCell>
                </TableRow>
              ) : null}
              {ordered.map((rule, index) => {
                const pending = pendingRecategorization(rule, transactions).length;
                return (
                  <TableRow
                    key={rule.id}
                    data-testid={`rule-${rule.id}`}
                    data-name={rule.name}
                    className={rule.enabled ? undefined : "text-muted-foreground"}
                  >
                    <TableCell className="money text-muted-foreground" data-testid="order">
                      {index + 1}
                    </TableCell>
                    <TableHead
                      scope="row"
                      className={cn(
                        "h-auto py-2 whitespace-normal",
                        rule.enabled ? "text-foreground" : "text-muted-foreground",
                      )}
                    >
                      <div className="grid gap-0.5">
                        <div className="flex flex-wrap items-center gap-2">
                          <span data-testid="name" className="font-medium">
                            {rule.name}
                          </span>
                          {rule.enabled ? null : <Badge variant="outline">disabled</Badge>}
                        </div>
                        <span
                          className="block text-xs font-normal text-muted-foreground"
                          data-testid="sentence"
                        >
                          When {describeConditions(rule, names, currency)},{" "}
                          {describeActions(rule, names)}.
                        </span>
                      </div>
                    </TableHead>
                    <TableCell className="money" data-testid="match-count">
                      {countMatches(rule, transactions)}
                    </TableCell>
                    <TableCell
                      className={cn("money", pending > 0 && rule.enabled && "text-warning")}
                      data-testid="pending"
                    >
                      {pending}
                    </TableCell>
                    <TableCell className="whitespace-normal">
                      <div className="flex flex-wrap justify-end gap-1">
                        <Button
                          variant="ghost"
                          size="sm"
                          className={ROW_ACTION}
                          disabled={index === 0}
                          onClick={() => move(rule, "up")}
                        >
                          Move up
                        </Button>
                        <Button
                          variant="ghost"
                          size="sm"
                          className={ROW_ACTION}
                          disabled={index >= ordered.length - 1}
                          onClick={() => move(rule, "down")}
                        >
                          Move down
                        </Button>
                        <Button
                          variant="ghost"
                          size="sm"
                          className={ROW_ACTION}
                          onClick={() => apply(rule)}
                        >
                          Apply
                        </Button>
                        <Button
                          variant="ghost"
                          size="sm"
                          className={ROW_ACTION}
                          onClick={() =>
                            void attempt(
                              (writes) => writes.updateRule(rule.id, { enabled: !rule.enabled }),
                              "The rule could not be changed.",
                            )
                          }
                        >
                          {rule.enabled ? "Disable" : "Enable"}
                        </Button>
                        <Button
                          variant="ghost"
                          size="sm"
                          className={ROW_ACTION}
                          onClick={() => setDraft(draftFromRule(rule, currency))}
                        >
                          Edit
                        </Button>
                        <Button
                          variant="ghost"
                          size="sm"
                          className={cn(ROW_ACTION, "text-destructive hover:text-destructive")}
                          onClick={() =>
                            void attempt(
                              (writes) => writes.deleteRule(rule.id),
                              "The rule could not be deleted.",
                            )
                          }
                        >
                          Delete
                        </Button>
                      </div>
                    </TableCell>
                  </TableRow>
                );
              })}
            </TableBody>
          </Table>
        </CardContent>
      </Card>
    </section>
  );
}
