import { type FormEvent, useState } from "react";

import type { RationalApp } from "../data/rational.js";
import type { ScopeSession } from "../data/scope.js";
import type { HouseholdCollectionId, Rule } from "../model/types.js";
import { amountToText, parseAmount } from "../selectors/money.js";
import { countMatches, pendingRecategorization, sortRules } from "../selectors/rules.js";
import { useQuery } from "./hooks.js";

/**
 * Categorization rules.
 *
 * A rule says how a transaction should be filed, and the editor says how many
 * of the household's transactions it would touch before it touches any: a
 * rule is written against real data, so the count is the feedback that makes
 * it writable. Applying one is a separate, explicit action.
 */
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
  const categories = useQuery(
    session
      .collection("taxonomy")
      ?.find({ selector: { kind: "category" }, sort: [{ name: "asc" }] }) ?? null,
  );
  const [problem, setProblem] = useState<string | null>(null);
  const [applied, setApplied] = useState<string | null>(null);

  const create = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const form = event.currentTarget;
    const data = new FormData(form);
    try {
      const minimum = String(data.get("amount_min") ?? "").trim();
      const maximum = String(data.get("amount_max") ?? "").trim();
      await app.writes?.createRule({
        name: String(data.get("name") ?? "").trim(),
        match: {
          description_contains: String(data.get("description_contains") ?? "").trim(),
          ...(minimum === "" ? {} : { amount_min: parseAmount(minimum, currency) }),
          ...(maximum === "" ? {} : { amount_max: parseAmount(maximum, currency) }),
          account_id: String(data.get("account_id") ?? ""),
        },
        set_category_id: String(data.get("set_category_id") ?? ""),
        priority: Number(data.get("priority") ?? 10),
      });
      form.reset();
      setProblem(null);
    } catch (error) {
      setProblem(error instanceof Error ? error.message : "the rule could not be saved");
    }
  };

  const apply = async (rule: Rule) => {
    const pending = pendingRecategorization(rule, transactions);
    for (const transaction of pending) {
      // Only stored transactions reach here, so each one has an id; the
      // shared shape leaves it optional because a rule can also be tried
      // against a row an import has not written yet.
      if (transaction.id === undefined) continue;
      await app.writes?.updateTransaction(transaction.id, {
        category_id: rule.set_category_id ?? null,
        rule_id: rule.id,
      });
    }
    await app.writes?.updateRule(rule.id, { match_count: countMatches(rule, transactions) });
    setApplied(
      pending.length === 0
        ? `${rule.name} already agrees with every transaction it matches.`
        : `${rule.name} recategorized ${pending.length} transactions.`,
    );
  };

  const categoryName = (id: string | undefined) =>
    id === undefined || id === "" ? "—" : (categories.find((entry) => entry.id === id)?.name ?? id);

  return (
    <section aria-labelledby="rules-title" data-testid="rules-screen">
      <div className="section-heading">
        <h2 id="rules-title">Rules</h2>
      </div>
      {problem === null ? null : (
        <p className="notice error" role="alert">
          {problem}
        </p>
      )}
      {applied === null ? null : (
        <p className="notice success" role="status" data-testid="rule-applied">
          {applied}
        </p>
      )}

      <table className="data-table" aria-label="Rules">
        <thead>
          <tr>
            <th scope="col">Priority</th>
            <th scope="col">Rule</th>
            <th scope="col">Matches</th>
            <th scope="col">Category</th>
            <th scope="col">Would touch</th>
            <th scope="col">
              <span className="visually-hidden">Actions</span>
            </th>
          </tr>
        </thead>
        <tbody>
          {rules.length === 0 ? (
            <tr>
              <td colSpan={6} className="muted">
                No rules yet.
              </td>
            </tr>
          ) : null}
          {sortRules(rules)
            .map((sorted) => rules.find((rule) => rule.id === sorted.id))
            .filter((rule): rule is Rule => rule !== undefined)
            .map((rule) => (
              <tr key={rule.id} data-testid={`rule-${rule.id}`} data-name={rule.name}>
                <td>{rule.priority}</td>
                <th scope="row">{rule.name}</th>
                <td data-testid="match-count">{countMatches(rule, transactions)}</td>
                <td>{categoryName(rule.set_category_id)}</td>
                <td data-testid="pending">{pendingRecategorization(rule, transactions).length}</td>
                <td className="actions">
                  <button type="button" className="link" onClick={() => void apply(rule)}>
                    Apply
                  </button>
                  <button
                    type="button"
                    className="link"
                    onClick={() => void app.writes?.updateRule(rule.id, { enabled: !rule.enabled })}
                  >
                    {rule.enabled ? "Disable" : "Enable"}
                  </button>
                  <button
                    type="button"
                    className="link"
                    onClick={() => void app.writes?.deleteRule(rule.id)}
                  >
                    Delete
                  </button>
                </td>
              </tr>
            ))}
        </tbody>
      </table>

      <form aria-label="New rule" onSubmit={(event) => void create(event)}>
        <h3>New rule</h3>
        <label>
          Name
          <input name="name" required maxLength={200} />
        </label>
        <label>
          Description contains
          <input name="description_contains" maxLength={500} />
        </label>
        <label>
          Amount at least
          <input
            name="amount_min"
            inputMode="decimal"
            placeholder={amountToText(-5_000, currency)}
          />
        </label>
        <label>
          Amount at most
          <input name="amount_max" inputMode="decimal" placeholder={amountToText(-100, currency)} />
        </label>
        <label>
          Account
          <select name="account_id" defaultValue="">
            <option value="">Any account</option>
            {accounts.map((account) => (
              <option key={account.id} value={account.id}>
                {account.name}
              </option>
            ))}
          </select>
        </label>
        <label>
          File as
          <select name="set_category_id" defaultValue="">
            <option value="">Leave uncategorized</option>
            {categories.map((category) => (
              <option key={category.id} value={category.id}>
                {category.name}
              </option>
            ))}
          </select>
        </label>
        <label>
          Priority
          <input name="priority" type="number" min={0} max={1000} defaultValue={10} />
        </label>
        <button type="submit">Add rule</button>
      </form>
    </section>
  );
}
