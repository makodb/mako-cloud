import { type FormEvent, useMemo, useState } from "react";

import type { RationalApp } from "../data/rational.js";
import type { ScopeSession } from "../data/scope.js";
import { BALANCE_UPDATE_DESCRIPTION, ValidationError } from "../data/writes.js";
import {
  ACCOUNT_TYPE_LABELS,
  type Account,
  HOLDING_TYPES,
  type HouseholdCollectionId,
  isCategory,
  TRACKED_TYPES,
  type Transaction,
} from "../model/types.js";
import { accountBalance, holdingsValue } from "../selectors/balances.js";
import {
  type HistoryRangeKey,
  historyRange,
  selectAccountBalanceHistory,
} from "../selectors/history.js";
import { type HoldingRow, holdingGain, holdingValue } from "../selectors/holdings.js";
import { merchantResolver } from "../selectors/merchants.js";
import { amountToText, formatMinorUnits, parseAmount } from "../selectors/money.js";
import { sortTransactions } from "../selectors/transactions.js";
import {
  AccountForm,
  HistoryRangePicker,
  memberLabel,
  todayIso,
  useHouseholdMembers,
} from "./account-form.js";
import { LineChart } from "./charts/index.js";
import { HoldingForm, HoldingsTable } from "./holdings.js";
import { useBehavior, useQuery } from "./hooks.js";
import { routeHash, transactionsHash } from "./router.js";
import "./styles/accounts.css";

/** How many of an account's transactions the page lists before pointing at the full list. */
const TRANSACTION_LIMIT = 100;

/**
 * One account: what it is, what it holds, how its balance has moved night by
 * night, and what happened in it. The balance is derived here exactly as the
 * accounts list derives it; the history is what the nightly snapshots
 * recorded of this account, never recomputed from today's transactions, so a
 * transaction edited last week does not rewrite last week.
 */
export function AccountScreen({
  app,
  session,
  currency,
  accountId,
}: {
  app: RationalApp;
  session: ScopeSession<HouseholdCollectionId>;
  currency: string;
  accountId: string;
}) {
  const state = useBehavior(app.state$);
  const accounts = useQuery(
    session.collection("accounts")?.find({ sort: [{ name: "asc" }] }) ?? null,
  );
  const account = accounts.find((candidate) => candidate.id === accountId);
  const transactions = useQuery(
    session.collection("transactions")?.find({ selector: { account_id: accountId } }) ?? null,
  );
  const snapshots = useQuery(session.collection("net_worth_snapshots")?.find() ?? null);
  const taxonomy = useQuery(session.collection("taxonomy")?.find() ?? null);
  const connections = useQuery(
    session.collection("connections")?.find({ selector: { account_id: accountId } }) ?? null,
  );
  const members = useHouseholdMembers(app);
  const [rangeKey, setRangeKey] = useState<HistoryRangeKey>("3M");
  const today = todayIso();
  const range = useMemo(() => historyRange(rangeKey, today), [rangeKey, today]);
  const history = selectAccountBalanceHistory(snapshots, accountId, range);
  const sorted = useMemo(() => sortTransactions(transactions), [transactions]);
  const resolveMerchant = useMemo(() => merchantResolver(taxonomy), [taxonomy]);
  const categoryNames = useMemo(
    () => new Map(taxonomy.filter(isCategory).map((category) => [category.id, category.name])),
    [taxonomy],
  );
  const balance = useMemo(
    () => (account === undefined ? 0 : accountBalance(account, transactions)),
    [account, transactions],
  );
  const holdingRows = useMemo<readonly HoldingRow[]>(
    () =>
      account === undefined
        ? []
        : (account.holdings ?? []).map((holding) => ({
            account,
            holding,
            value: holdingValue(holding),
            gain: holdingGain(holding),
          })),
    [account],
  );
  const [panel, setPanel] = useState<"edit" | "value" | "holding" | null>(null);
  const [editingHolding, setEditingHolding] = useState<HoldingRow | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const role = app.roleIn(state.currentHouseholdId);
  const canEdit = role === "owner" || role === "editor";

  if (account === undefined) {
    return (
      <section aria-labelledby="account-title" data-testid="account-screen">
        <nav className="breadcrumb" aria-label="Breadcrumb">
          <a href={routeHash({ name: "accounts" })}>Accounts</a>
        </nav>
        <div className="heading">
          <h1 id="account-title">Account</h1>
        </div>
        {accounts.length === 0 ? (
          <p role="status" className="muted">
            Opening the account…
          </p>
        ) : (
          <p className="muted" data-testid="account-missing">
            There is no account with this id on this device. It may have been deleted, or belong to
            another space.
          </p>
        )}
      </section>
    );
  }

  const tracked = TRACKED_TYPES.includes(account.type);
  const holdsPositions = HOLDING_TYPES.includes(account.type);
  const hidden = account.hide_from_net_worth === true;
  const closed = account.closed_at !== undefined;
  const positionsValue = holdingsValue(account.holdings);
  const owner =
    account.owner_id === undefined
      ? null
      : (members.find((membership) => membership.user_id === account.owner_id) ?? null);
  const institutionConnections = connections.filter(
    (connection) => connection.kind === "institution" || connection.kind === "plaid",
  );
  const listed = sorted.slice(0, TRANSACTION_LIMIT);

  const act = async (action: () => Promise<unknown> | undefined) => {
    setProblem(null);
    try {
      await action();
    } catch (caught) {
      setProblem(caught instanceof Error ? caught.message : "That could not be saved.");
    }
  };

  return (
    <section aria-labelledby="account-title" data-testid="account-screen">
      <nav className="breadcrumb" aria-label="Breadcrumb">
        <a href={routeHash({ name: "accounts" })}>Accounts</a>
      </nav>
      <div className="heading">
        <div>
          <h1 id="account-title">{account.name}</h1>
          <p className="muted account-meta" data-testid="account-meta">
            <span data-testid="type">{ACCOUNT_TYPE_LABELS[account.type]}</span>
            {account.institution === undefined ? null : <span>{account.institution}</span>}
            <span>{account.currency}</span>
            {account.owner_id === undefined ? null : (
              <span data-testid="owner">
                {owner === null ? account.owner_id : memberLabel(owner)}&apos;s
              </span>
            )}
            {hidden ? <span className="chip">hidden from net worth</span> : null}
            {closed ? <span className="chip">closed</span> : null}
          </p>
        </div>
        {canEdit ? (
          <div className="account-actions">
            <button type="button" className="secondary" onClick={() => setPanel("edit")}>
              Edit
            </button>
            <button
              type="button"
              className="secondary"
              onClick={() => void act(() => app.writes?.setHideFromNetWorth(account.id, !hidden))}
            >
              {hidden ? "Unhide from net worth" : "Hide from net worth"}
            </button>
            {tracked ? (
              <button type="button" onClick={() => setPanel("value")}>
                Update value
              </button>
            ) : null}
            {holdsPositions ? (
              <button
                type="button"
                onClick={() => {
                  setEditingHolding(null);
                  setPanel("holding");
                }}
              >
                Add holding
              </button>
            ) : null}
            <button
              type="button"
              className="secondary"
              onClick={() =>
                void act(() =>
                  closed
                    ? app.writes?.reopenAccount(account.id)
                    : app.writes?.closeAccount(account.id),
                )
              }
            >
              {closed ? "Reopen account" : "Close account"}
            </button>
          </div>
        ) : null}
      </div>
      {problem === null ? null : (
        <p className="notice error" role="alert">
          {problem}
        </p>
      )}

      <div className="totals">
        <div className="total" data-testid="account-balance">
          <span>Balance</span>
          <strong data-testid="balance">{formatMinorUnits(balance, account.currency)}</strong>
          <small>
            {tracked
              ? "Tracked value, updated in place"
              : `Opened ${account.opening_date} at ${formatMinorUnits(
                  account.opening_balance,
                  account.currency,
                )}`}
          </small>
        </div>
        {holdsPositions ? (
          <>
            <div className="total">
              <span>Cash</span>
              <strong data-testid="cash">
                {formatMinorUnits(balance - positionsValue, account.currency)}
              </strong>
            </div>
            <div className="total">
              <span>Holdings</span>
              <strong data-testid="holdings-value">
                {formatMinorUnits(positionsValue, account.currency)}
              </strong>
              <small>
                {holdingRows.length} {holdingRows.length === 1 ? "position" : "positions"}
              </small>
            </div>
          </>
        ) : null}
      </div>

      {panel === "edit" ? (
        <AccountForm
          app={app}
          account={account}
          defaultCurrency={currency}
          onDone={() => setPanel(null)}
        />
      ) : null}
      {panel === "value" && tracked ? (
        <UpdateValueForm
          app={app}
          account={account}
          current={balance}
          onDone={() => setPanel(null)}
        />
      ) : null}
      {panel === "holding" && holdsPositions ? (
        <HoldingForm
          key={editingHolding?.holding.id ?? "new"}
          app={app}
          accounts={[account]}
          accountId={account.id}
          holding={editingHolding}
          onDone={() => setPanel(null)}
        />
      ) : null}

      <div className="section-heading">
        <h2>Balance history</h2>
        <HistoryRangePicker value={rangeKey} onChange={setRangeKey} />
      </div>
      {history.length === 0 ? (
        <p className="muted" data-testid="balance-history-empty">
          No snapshots for this account in this range. The nightly job records one a night, so the
          chart fills in from tomorrow.
        </p>
      ) : (
        <div className="history" data-testid="balance-history">
          <LineChart
            points={history.map((point) => ({ x: point.date, y: point.balance }))}
            currency={account.currency}
            ariaLabel={`Balance of ${account.name} over time`}
            label="Balance"
            showArea
            height={180}
          />
          <p className="muted" data-testid="balance-history-change">
            {balanceCaption(history, account.currency)}
          </p>
        </div>
      )}

      {holdsPositions ? (
        <>
          <h2>Holdings</h2>
          <HoldingsTable
            app={app}
            rows={holdingRows}
            showAccount={false}
            canEdit={canEdit}
            onEdit={(row) => {
              setEditingHolding(row);
              setPanel("holding");
            }}
            emptyMessage="No holdings yet. Add one and its value joins the balance."
          />
        </>
      ) : null}

      {institutionConnections.length === 0 ? null : (
        <>
          <h2>Connection</h2>
          {institutionConnections.map((connection) => (
            <p key={connection.id} className="hint" data-testid="connection-status">
              Connected to {connection.institution ?? "an institution"} —{" "}
              <span data-testid="status">{connection.status ?? "connected"}</span>; last sync{" "}
              {connection.last_sync_at === undefined
                ? "never"
                : new Date(connection.last_sync_at).toLocaleString()}
              {connection.last_sync_outcome === undefined
                ? ""
                : ` (${connection.last_sync_outcome})`}
              . <a href={routeHash({ name: "settings", page: "connections" })}>Connections</a>
            </p>
          ))}
        </>
      )}

      <div className="section-heading">
        <h2>{tracked ? "Balance updates" : "Transactions"}</h2>
        <a href={transactionsHash({ account: account.id })}>All transactions</a>
      </div>
      <table className="list" aria-label={tracked ? "Balance updates" : "Transactions"}>
        <thead>
          <tr>
            <th scope="col">Date</th>
            <th scope="col">Description</th>
            <th scope="col">Category</th>
            <th scope="col" className="amount">
              Amount
            </th>
          </tr>
        </thead>
        <tbody>
          {listed.length === 0 ? (
            <tr>
              <td colSpan={4} className="empty">
                {tracked ? "No balance updates yet." : "No transactions yet."}
              </td>
            </tr>
          ) : null}
          {listed.map((transaction) => (
            <TransactionRow
              key={transaction.id}
              transaction={transaction}
              merchant={
                transaction.adjustment === true
                  ? BALANCE_UPDATE_DESCRIPTION
                  : resolveMerchant(transaction).name
              }
              categoryName={categoryNames}
            />
          ))}
        </tbody>
      </table>
      {sorted.length > listed.length ? (
        <p className="hint">
          Showing the latest {listed.length} of {sorted.length}.{" "}
          <a href={transactionsHash({ account: account.id })}>All transactions</a>
        </p>
      ) : null}
    </section>
  );
}

/** "12 snapshots, 2026-06-06 to 2026-08-15: up $1,200.00." -- or the one-snapshot form. */
function balanceCaption(
  history: ReadonlyArray<{ readonly date: string; readonly balance: number }>,
  currency: string,
): string {
  const first = history[0];
  const last = history[history.length - 1];
  if (first === undefined || last === undefined) return "";
  if (history.length === 1) return `One snapshot, ${formatMinorUnits(first.balance, currency)}.`;
  const change = last.balance - first.balance;
  return `${history.length} snapshots, ${first.date} to ${last.date}: ${
    change >= 0 ? "up" : "down"
  } ${formatMinorUnits(Math.abs(change), currency)}.`;
}

/**
 * The same row the transactions list draws: when, with whom, filed where,
 * how much. A balance update, a transfer leg, and a hidden transaction say
 * what they are, since none of them is money the household spent.
 */
function TransactionRow({
  transaction,
  merchant,
  categoryName,
}: {
  transaction: Transaction;
  merchant: string;
  categoryName: ReadonlyMap<string, string>;
}) {
  const needsReview =
    transaction.reviewed !== true &&
    (transaction.external_id !== undefined || transaction.import_batch_id !== undefined);
  return (
    <tr
      data-testid={`transaction-${transaction.id}`}
      data-description={transaction.description}
      className={
        transaction.hidden === true || transaction.adjustment === true ? "muted" : undefined
      }
    >
      <td>{transaction.date}</td>
      <td>
        <span data-testid="description">{merchant}</span>
        {merchant === transaction.description ? null : <small> {transaction.description}</small>}
        {transaction.notes === undefined ? null : <small> {transaction.notes}</small>}
      </td>
      <td>
        {transaction.splits.length > 0
          ? "split"
          : transaction.category_id === undefined
            ? ""
            : (categoryName.get(transaction.category_id) ?? transaction.category_id)}
        {transaction.adjustment === true ? <span className="chip"> balance update</span> : null}
        {transaction.transfer_id === undefined ? null : <span className="chip"> transfer</span>}
        {transaction.hidden === true && transaction.adjustment !== true ? (
          <span className="chip"> hidden</span>
        ) : null}
        {needsReview ? <span className="chip"> needs review</span> : null}
      </td>
      <td className="amount" data-testid="amount">
        {formatMinorUnits(transaction.amount, transaction.currency)}
      </td>
    </tr>
  );
}

/**
 * Move a tracked account to a new value. The difference is booked by the
 * write helper as a balance update -- a hidden, flagged transaction -- so the
 * account and net worth move while cash flow and budgets never see spending.
 */
function UpdateValueForm({
  app,
  account,
  current,
  onDone,
}: {
  app: RationalApp;
  account: Account;
  current: number;
  onDone: () => void;
}) {
  const [value, setValue] = useState(amountToText(current, account.currency));
  const [date, setDate] = useState(todayIso());
  const [error, setError] = useState<string | null>(null);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setError(null);
    const writes = app.writes;
    if (writes === null) return;
    try {
      await writes.updateTrackedBalance(account.id, parseAmount(value, account.currency), date);
      onDone();
    } catch (caught) {
      setError(
        caught instanceof ValidationError || caught instanceof RangeError
          ? caught.message
          : "The value could not be saved.",
      );
    }
  };

  return (
    <form
      className="editor"
      aria-label="Update value"
      data-testid="update-value"
      onSubmit={(event) => void submit(event)}
    >
      <h2>Update value</h2>
      <div className="grid">
        <label>
          New value ({account.currency})
          <input
            name="value"
            inputMode="decimal"
            required
            value={value}
            onChange={(event) => setValue(event.target.value)}
          />
        </label>
        <label>
          As of
          <input
            name="date"
            type="date"
            required
            value={date}
            onChange={(event) => setDate(event.target.value)}
          />
        </label>
      </div>
      <p className="hint">
        The difference from {formatMinorUnits(current, account.currency)} is booked as a balance
        update: it moves this account and net worth, and never counts as income or spending.
      </p>
      {error === null ? null : (
        <p className="error" role="alert">
          {error}
        </p>
      )}
      <div className="actions">
        <button type="submit">Save value</button>
        <button type="button" className="secondary" onClick={onDone}>
          Cancel
        </button>
      </div>
    </form>
  );
}
