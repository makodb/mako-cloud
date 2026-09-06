import { useMemo } from "react";

import type { RationalApp } from "../data/rational.js";
import type { ScopeSession } from "../data/scope.js";
import type { HouseholdCollectionId } from "../model/types.js";
import { selectAccountBalances } from "../selectors/balances.js";
import { accountsCsv, transactionsCsv } from "../selectors/export.js";
import { sortTransactions } from "../selectors/transactions.js";
import { useBehavior, useQuery } from "./hooks.js";
import "./styles/settings-pages.css";

/**
 * The way out: the household's accounts and transactions as CSV files.
 *
 * Both files are built here, from the same replicated documents every other
 * screen reads, and handed to the browser as a download -- nothing is asked
 * of the server, so the export works offline like the rest of the app. The
 * rows can only be this household's: each household is its own database on
 * the device, replicated from collections the policy scopes to its members,
 * so there is no other household's row for a filter to miss.
 */
export function DataScreen({
  app,
  session,
  currency,
}: {
  app: RationalApp;
  session: ScopeSession<HouseholdCollectionId>;
  currency: string;
}) {
  const state = useBehavior(app.state$);
  const household =
    state.households.find((candidate) => candidate.id === state.currentHouseholdId) ?? null;
  const accounts = useQuery(session.collection("accounts")?.find() ?? null);
  const transactions = useQuery(session.collection("transactions")?.find() ?? null);
  const taxonomy = useQuery(session.collection("taxonomy")?.find() ?? null);
  const balances = selectAccountBalances(accounts, transactions);
  // Newest first, as the transactions screen lists them; a spreadsheet sorts
  // either way, and a file that opens on last week reads as current.
  const orderedTransactions = useMemo(() => sortTransactions(transactions), [transactions]);
  const orderedAccounts = useMemo(
    () =>
      [...accounts].sort(
        (left, right) => left.name.localeCompare(right.name) || left.id.localeCompare(right.id),
      ),
    [accounts],
  );
  const today = new Date().toISOString().slice(0, 10);

  const exportTransactions = () => {
    download(
      `rational-transactions-${today}.csv`,
      transactionsCsv(orderedTransactions, { accounts, taxonomy }),
    );
  };

  const exportAccounts = () => {
    download(`rational-accounts-${today}.csv`, accountsCsv(orderedAccounts, balances));
  };

  return (
    <section aria-labelledby="data-title" data-testid="data-screen">
      <div className="heading">
        <h1 id="data-title">Data</h1>
      </div>
      <p className="hint">
        Every row in these files belongs to {household?.name ?? "this household"} and to nobody
        else: a household's collections hold only its own documents, so there is nothing of anyone
        else's for a filter to leave out. Amounts are decimal text in each row's own currency — the
        household's is {currency} — and names stand in for ids.
      </p>
      <div className="exports">
        <div className="export-card" data-testid="export-transactions">
          <h2>Transactions</h2>
          <p className="hint">
            One row per transaction: date, account, description, merchant, category and its group,
            tags, amount, currency, notes, whether it is hidden, and the transfer it belongs to.
          </p>
          <p className="rows" data-testid="transactions-row-count">
            {rowCount(orderedTransactions.length)}
          </p>
          <button type="button" onClick={exportTransactions} disabled={transactions.length === 0}>
            Export transactions
          </button>
        </div>
        <div className="export-card" data-testid="export-accounts">
          <h2>Accounts</h2>
          <p className="hint">
            One row per account, closed ones included: name, class, type, currency, today's balance,
            institution, and whether it is hidden from net worth or closed.
          </p>
          <p className="rows" data-testid="accounts-row-count">
            {rowCount(orderedAccounts.length)}
          </p>
          <button type="button" onClick={exportAccounts} disabled={accounts.length === 0}>
            Export accounts
          </button>
        </div>
      </div>
    </section>
  );
}

function rowCount(count: number): string {
  return `${count} ${count === 1 ? "row" : "rows"}`;
}

/**
 * Hand the browser a file. A blob URL on an anchor with `download` is what a
 * browser treats as saving rather than navigating, and it works without a
 * server -- the published demo has none. The URL is revoked once the
 * download has had time to start; revoking on the same tick can cut it off.
 */
function download(filename: string, text: string): void {
  const blob = new Blob([text], { type: "text/csv;charset=utf-8" });
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = filename;
  anchor.rel = "noopener";
  anchor.hidden = true;
  document.body.append(anchor);
  anchor.click();
  anchor.remove();
  window.setTimeout(() => URL.revokeObjectURL(url), 10_000);
}
