import type { RationalApp } from "../data/rational.js";
import type { ScopeSession } from "../data/scope.js";
import type { HouseholdCollectionId } from "../model/types.js";
import { accountBalances, netWorthByCurrency } from "../selectors/balances.js";
import { formatMinorUnits } from "../selectors/money.js";
import {
  netWorthPath,
  selectCashFlow,
  selectNetWorthHistories,
  selectSpendingByAccount,
  selectSpendingByCategory,
  selectSpendingByMonth,
} from "../selectors/reports.js";
import { selectAvailableMonths } from "../selectors/transactions.js";
import { useQuery } from "./hooks.js";
import { type Route, routeHash } from "./router.js";

/**
 * Every number here is computed on the device, over the documents replication
 * already delivered. There is no aggregation API: the totals update the
 * moment a local write lands, they are right offline, and they are the ones
 * the person can also see in the list. What that costs is measured rather
 * than assumed -- see `test-unit/reports.test.mjs` and the findings log.
 */
export function ReportsScreen({
  app,
  session,
  route,
}: {
  app: RationalApp;
  session: ScopeSession<HouseholdCollectionId>;
  route: Extract<Route, { name: "reports" }>;
}) {
  const transactions = useQuery(session.collection("transactions")?.find() ?? null);
  const snapshots = useQuery(session.collection("net_worth_snapshots")?.find() ?? null);
  const accounts = useQuery(
    session.collection("accounts")?.find({ sort: [{ name: "asc" }] }) ?? null,
  );
  const categories = useQuery(
    session.collection("taxonomy")?.find({ selector: { kind: "category" } }) ?? null,
  );
  const months = selectAvailableMonths(transactions);
  const month = route.month;
  const balances = accountBalances(accounts, transactions);
  const netWorth = netWorthByCurrency(accounts, balances);
  const cashFlow = selectCashFlow(transactions);
  const byCategory = selectSpendingByCategory(transactions, month);
  const byAccount = selectSpendingByAccount(transactions, month);
  const byMonth = selectSpendingByMonth(transactions);
  const histories = selectNetWorthHistories(snapshots);
  const categoryName = (id: string) =>
    id === "" ? "uncategorized" : (categories.find((entry) => entry.id === id)?.name ?? id);
  const accountName = (id: string) => accounts.find((account) => account.id === id)?.name ?? id;

  return (
    <section aria-labelledby="reports-title" data-testid="reports-screen">
      <div className="section-heading">
        <div>
          <h2 id="reports-title">Reports</h2>
          <p className="muted" data-testid="reports-stamp">
            {app.state.household?.syncedAt === null || app.state.household === null
              ? "Not synced yet — these are your local records."
              : `As of the last sync, ${new Date(app.state.household.syncedAt).toLocaleTimeString()}.`}
          </p>
        </div>
        <label>
          Month
          <select
            value={month ?? ""}
            onChange={(event) => {
              const next = event.target.value;
              window.location.hash = routeHash(
                next === "" ? { name: "reports" } : { name: "reports", month: next },
              );
            }}
          >
            <option value="">Every month</option>
            {months.map((available) => (
              <option key={available} value={available}>
                {available}
              </option>
            ))}
          </select>
        </label>
      </div>

      <h3>Net worth</h3>
      {netWorth.length === 0 ? (
        <p className="muted">No open accounts yet.</p>
      ) : (
        <table className="data-table" aria-label="Net worth">
          <thead>
            <tr>
              <th scope="col">Currency</th>
              <th scope="col">Assets</th>
              <th scope="col">Liabilities</th>
              <th scope="col">Net worth</th>
            </tr>
          </thead>
          <tbody>
            {netWorth.map((row) => (
              <tr key={row.currency} data-testid={`net-worth-${row.currency}`}>
                <th scope="row">{row.currency}</th>
                <td>{formatMinorUnits(row.assets, row.currency)}</td>
                <td>{formatMinorUnits(row.liabilities, row.currency)}</td>
                <td data-testid="net">{formatMinorUnits(row.netWorth, row.currency)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      <h3>Net worth over time</h3>
      {histories.length === 0 ? (
        <p className="muted" data-testid="net-worth-history-empty">
          No snapshots yet. The nightly job records one a night, so this fills in from tomorrow.
        </p>
      ) : (
        histories.map((history) => (
          <div key={history.currency} data-testid={`net-worth-history-${history.currency}`}>
            <svg
              className="sparkline"
              viewBox="0 0 100 100"
              preserveAspectRatio="none"
              role="img"
              aria-label={`Net worth from ${history.points[0]?.date} to ${
                history.points[history.points.length - 1]?.date
              }, ${formatMinorUnits(history.low, history.currency)} to ${formatMinorUnits(
                history.high,
                history.currency,
              )}`}
            >
              <polyline points={netWorthPath(history)} fill="none" strokeWidth="2" />
            </svg>
            <p className="muted" data-testid="net-worth-change">
              {history.points.length === 1
                ? `One snapshot, ${formatMinorUnits(history.points[0]?.netWorth ?? 0, history.currency)}.`
                : `${history.points.length} snapshots, ${history.points[0]?.date} to ${
                    history.points[history.points.length - 1]?.date
                  }: ${history.change >= 0 ? "up" : "down"} ${formatMinorUnits(
                    Math.abs(history.change),
                    history.currency,
                  )}.`}
            </p>
          </div>
        ))
      )}

      <h3>Cash flow</h3>
      {cashFlow.length === 0 ? (
        <p className="muted">No transactions yet.</p>
      ) : (
        <table className="data-table" aria-label="Cash flow">
          <thead>
            <tr>
              <th scope="col">Month</th>
              <th scope="col">In</th>
              <th scope="col">Out</th>
              <th scope="col">Net</th>
            </tr>
          </thead>
          <tbody>
            {cashFlow.map((row) => (
              <tr key={`${row.month}:${row.currency}`} data-testid={`cash-flow-${row.month}`}>
                <th scope="row">{row.month}</th>
                <td>{formatMinorUnits(row.income, row.currency)}</td>
                <td>{formatMinorUnits(row.expense, row.currency)}</td>
                <td data-testid="net">{formatMinorUnits(row.net, row.currency)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      <h3>Spending by category</h3>
      <Breakdown
        label="Spending by category"
        slices={byCategory}
        name={categoryName}
        testId="category"
      />

      <h3>Spending by account</h3>
      <Breakdown
        label="Spending by account"
        slices={byAccount}
        name={accountName}
        testId="account"
      />

      <h3>Spending by month</h3>
      <Breakdown label="Spending by month" slices={byMonth} name={(key) => key} testId="month" />
    </section>
  );
}

function Breakdown({
  label,
  slices,
  name,
  testId,
}: {
  label: string;
  slices: ReadonlyArray<{ key: string; currency: string; amount: number }>;
  name: (key: string) => string;
  testId: string;
}) {
  if (slices.length === 0) {
    return <p className="muted">Nothing spent yet.</p>;
  }
  return (
    <table className="data-table" aria-label={label}>
      <thead>
        <tr>
          <th scope="col">{label.replace("Spending by ", "")}</th>
          <th scope="col">Spent</th>
        </tr>
      </thead>
      <tbody>
        {slices.map((slice) => (
          <tr key={`${slice.key}:${slice.currency}`} data-testid={`${testId}-${slice.key}`}>
            <th scope="row">{name(slice.key)}</th>
            <td data-testid="amount">{formatMinorUnits(slice.amount, slice.currency)}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}
