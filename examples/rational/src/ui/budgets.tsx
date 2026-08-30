import { type FormEvent, useState } from "react";

import type { RationalApp } from "../data/rational.js";
import type { ScopeSession } from "../data/scope.js";
import type { HouseholdCollectionId } from "../model/types.js";
import { budgetTotals, monthBudgets } from "../selectors/budgets.js";
import { amountToText, formatMinorUnits, parseAmount } from "../selectors/money.js";
import { monthKey } from "../selectors/transactions.js";
import { useQuery } from "./hooks.js";
import { type Route, routeHash } from "./router.js";

/**
 * Monthly budgets per category.
 *
 * A budget with rollover carries what its month did not spend into the next,
 * and carries an overspend forward as a smaller allowance -- which is what
 * makes it useful for the categories people actually roll over. The screen
 * shows the allowance and where it came from, because "40 of 120" is only
 * meaningful when it says which 120.
 */
export function BudgetsScreen({
  app,
  session,
  route,
  currency,
}: {
  app: RationalApp;
  session: ScopeSession<HouseholdCollectionId>;
  route: Extract<Route, { name: "budgets" }>;
  currency: string;
}) {
  const budgets = useQuery(session.collection("budgets")?.find() ?? null);
  const transactions = useQuery(session.collection("transactions")?.find() ?? null);
  const categories = useQuery(
    session
      .collection("taxonomy")
      ?.find({ selector: { kind: "category" }, sort: [{ name: "asc" }] }) ?? null,
  );
  const [problem, setProblem] = useState<string | null>(null);
  const month = route.month ?? monthKey(new Date().toISOString().slice(0, 10));
  const statuses = monthBudgets(budgets, transactions, month);
  const totals = budgetTotals(statuses);
  const categoryName = (id: string) => categories.find((entry) => entry.id === id)?.name ?? id;
  const months = [...new Set([month, ...budgets.map((budget) => budget.month)])].sort().reverse();

  const save = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const form = event.currentTarget;
    const data = new FormData(form);
    try {
      await app.writes?.setBudget({
        category_id: String(data.get("category_id") ?? ""),
        month,
        amount: parseAmount(String(data.get("amount") ?? ""), currency),
        currency,
        rollover: data.get("rollover") === "on",
      });
      form.reset();
      setProblem(null);
    } catch (error) {
      setProblem(error instanceof Error ? error.message : "the budget could not be saved");
    }
  };

  return (
    <section aria-labelledby="budgets-title" data-testid="budgets-screen">
      <div className="section-heading">
        <h2 id="budgets-title">Budgets</h2>
        <label>
          Month
          <select
            value={month}
            onChange={(event) => {
              window.location.hash = routeHash({ name: "budgets", month: event.target.value });
            }}
          >
            {months.map((available) => (
              <option key={available} value={available}>
                {available}
              </option>
            ))}
          </select>
        </label>
      </div>
      {problem === null ? null : (
        <p className="notice error" role="alert">
          {problem}
        </p>
      )}

      <table className="data-table" aria-label="Budgets">
        <thead>
          <tr>
            <th scope="col">Category</th>
            <th scope="col">Budget</th>
            <th scope="col">Carried in</th>
            <th scope="col">Allowance</th>
            <th scope="col">Spent</th>
            <th scope="col">Remaining</th>
            <th scope="col">
              <span className="visually-hidden">Actions</span>
            </th>
          </tr>
        </thead>
        <tbody>
          {statuses.length === 0 ? (
            <tr>
              <td colSpan={7} className="muted">
                No budgets for {month}.
              </td>
            </tr>
          ) : null}
          {statuses.map((status) => (
            <tr
              key={status.budget.id}
              data-testid={`budget-${status.budget.category_id}`}
              data-percent={status.percent}
            >
              <th scope="row">
                {categoryName(status.budget.category_id)}
                {status.budget.rollover ? <small> rolls over</small> : null}
              </th>
              <td>{formatMinorUnits(status.amount, status.budget.currency)}</td>
              <td data-testid="carried-in">
                {status.carriedIn === 0
                  ? "—"
                  : formatMinorUnits(status.carriedIn, status.budget.currency)}
              </td>
              <td data-testid="allowance">
                {formatMinorUnits(status.allowance, status.budget.currency)}
              </td>
              <td data-testid="spent">{formatMinorUnits(status.spent, status.budget.currency)}</td>
              <td data-testid="remaining" className={status.remaining < 0 ? "over" : undefined}>
                {formatMinorUnits(status.remaining, status.budget.currency)}
              </td>
              <td className="actions">
                <button
                  type="button"
                  className="link"
                  onClick={() =>
                    void app.writes?.deleteBudget(status.budget.category_id, status.budget.month)
                  }
                >
                  Remove
                </button>
              </td>
            </tr>
          ))}
        </tbody>
        {totals.length === 0 ? null : (
          <tfoot>
            {totals.map((total) => (
              <tr key={total.currency} data-testid={`budget-total-${total.currency}`}>
                <th scope="row">Total {total.currency}</th>
                <td />
                <td />
                <td>{formatMinorUnits(total.allowance, total.currency)}</td>
                <td>{formatMinorUnits(total.spent, total.currency)}</td>
                <td>{formatMinorUnits(total.remaining, total.currency)}</td>
                <td />
              </tr>
            ))}
          </tfoot>
        )}
      </table>

      <form aria-label="Set a budget" onSubmit={(event) => void save(event)}>
        <h3>Set a budget for {month}</h3>
        <label>
          Category
          <select name="category_id" defaultValue="" required>
            <option value="">Choose a category</option>
            {categories.map((category) => (
              <option key={category.id} value={category.id}>
                {category.name}
              </option>
            ))}
          </select>
        </label>
        <label>
          Amount
          <input
            name="amount"
            inputMode="decimal"
            required
            placeholder={amountToText(40_000, currency)}
          />
        </label>
        <label>
          <input name="rollover" type="checkbox" />
          Carry what is left into next month
        </label>
        <button type="submit">Save budget</button>
      </form>
    </section>
  );
}
